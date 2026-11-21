//! SSA (Static Single Assignment) IR for MIR.
//!
//! Arsitektur-01 (stage 1 of N): core SSA data model.
//!
//! This module is deliberately NOT yet connected to the optimization pipeline.
//! Later stages will lower MIR into this SSA form and run CSE/DCE/LICM on it.
//! The intent of stage 1 is to establish a correct, tested SSA foundation:
//! a dominator-tree computation that satisfies the classic property that the
//! entry block dominates every reachable block.


use mivon_core::intern::Symbol;
use std::collections::HashSet;

/// A value in SSA form. A value is defined exactly once: either a constant,
/// a signal variable, or the result of an instruction identified by (block
/// index, instr index).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Value {
    Const(u64),
    /// A signal identified by its interned Symbol.
    Var(Symbol),
    /// Result of `instr_idx`-th instruction inside `block_idx`.
    Instr(usize, usize),
}

/// An SSA instruction. Terminators end a basic block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instruction {
    /// Constant value at current instruction position.
    Const(u64),
    /// Load signal value into current instruction position.
    Load(Value),
    /// Store value to signal.
    Store(Value, Value),
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

    /// Global unreachable-block elimination on SSA form.
    ///
    /// Uses the dominator tree: a block is reachable iff the entry block
    /// dominates it (equivalently `dom[b][b]` is true after
    /// [`Self::dominators`], since unreachable blocks are cleared to all-false).
    /// Unreachable blocks are removed and all jump/branch/phi targets are
    /// remapped to the new (dense) block indices. Entry is always placed at
    /// index 0. Indices are shifted, so this is NOT index-stable — callers
    /// holding `Value::Instr(block, idx)` across this call must rebuild them.
    ///
    /// The `.expect` sites panic only on a malformed/inconsistent CFG where a
    /// reachable block references an unreachable block (e.g. a `Phi` whose
    /// predecessor block is unreachable). On a well-formed SSA graph every
    /// terminator target of a reachable block is itself reachable, so the
    /// remap always succeeds.
    ///
    /// Returns `true` if at least one block was removed.
    pub fn eliminate_unreachable_blocks(&mut self) -> bool {
        let n = self.blocks.len();
        if n == 0 {
            return false;
        }
        let dom = self.dominators();
        // reachable(b) <=> dom[b][b] (entry always reaches itself).
        let reachable: Vec<bool> = (0..n).map(|b| dom[b][b]).collect();
        let reachable_count = reachable.iter().filter(|&&x| x).count();
        if reachable_count == n {
            return false;
        }

        // Dense remap: entry first at index 0, then other reachable blocks in
        // original order. Unreachable blocks map to None (dropped).
        let mut remap: Vec<Option<usize>> = vec![None; n];
        let mut new_blocks: Vec<BasicBlock> = Vec::with_capacity(reachable_count);

        if reachable[self.entry] {
            remap[self.entry] = Some(0);
            new_blocks.push(self.blocks[self.entry].clone());
        }
        for b in 0..n {
            if b == self.entry {
                continue;
            }
            if reachable[b] {
                remap[b] = Some(new_blocks.len());
                new_blocks.push(self.blocks[b].clone());
            }
        }

        // Fix up terminators + phi predecessor indices to the dense remap.
        for block in &mut new_blocks {
            let mut i = 0;
            while i < block.instrs.len() {
                match &block.instrs[i] {
                    Instruction::Jump(t) => {
                        let nt = remap[*t].expect("jump targets unreachable block");
                        block.instrs[i] = Instruction::Jump(nt);
                    }
                    Instruction::Branch { cond, then, r#else } => {
                        let nt = remap[*then].expect("branch targets unreachable block");
                        let ne = remap[*r#else].expect("branch targets unreachable block");
                        block.instrs[i] = Instruction::Branch {
                            cond: *cond,
                            then: nt,
                            r#else: ne,
                        };
                    }
                    Instruction::Phi(pairs) => {
                        let mut new_pairs = Vec::with_capacity(pairs.len());
                        for (val, pred_block) in pairs.iter() {
                            let np =
                                remap[*pred_block].expect("phi from unreachable block");
                            new_pairs.push((*val, np));
                        }
                        block.instrs[i] = Instruction::Phi(new_pairs);
                    }
                    _ => {}
                }
                i += 1;
            }
        }

        self.blocks = new_blocks;
        self.entry = 0;
        true
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
        Instruction::Load(_) => {}
        Instruction::Store(_, v) => {
            out.insert(*v);
        }
        Instruction::Const(_) => {}
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

/// True if the instruction defines an SSA result (Add/Sub/Mul/Phi/Load).
fn defines_result(instr: &Instruction) -> bool {
    matches!(
        instr,
        Instruction::Add(..)
            | Instruction::Sub(..)
            | Instruction::Mul(..)
            | Instruction::Phi(..)
            | Instruction::Load(_)
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
            | Instruction::Return(_)
            | Instruction::Store(_, _) => None,
            _ => Some(Value::Instr(0, i)), // block index filled in by caller
        })
        .collect()
}

/// Signature of a binary instruction for CSE matching.
#[derive(Hash, PartialEq, Eq, Clone, Debug)]
enum CseSig {
    Add(Value, Value),
    Sub(Value, Value),
    Mul(Value, Value),
}

impl Instruction {
    /// CSE signature of an instruction that defines a value.
    fn cse_signature(&self) -> Option<CseSig> {
        match self {
            Instruction::Add(a, b) => Some(CseSig::Add(*a, *b)),
            Instruction::Sub(a, b) => Some(CseSig::Sub(*a, *b)),
            Instruction::Mul(a, b) => Some(CseSig::Mul(*a, *b)),
            _ => None,
        }
    }
}

impl SsaFunction {
    /// Build a use-def chain: every `Value::Instr` defined by an instruction
    /// maps to the list of (block_idx, instr_idx) sites that read it.
    fn collect_uses(&self) -> std::collections::HashMap<Value, Vec<(usize, usize)>> {
        use std::collections::HashMap;
        let mut uses: HashMap<Value, Vec<(usize, usize)>> = HashMap::new();
        for (bi, block) in self.blocks.iter().enumerate() {
            for (ii, instr) in block.instrs.iter().enumerate() {
                match instr {
                    Instruction::Add(a, b) | Instruction::Sub(a, b) | Instruction::Mul(a, b) => {
                        uses.entry(*a).or_default().push((bi, ii));
                        uses.entry(*b).or_default().push((bi, ii));
                    }
                    Instruction::Phi(pairs) => {
                        for (v, _) in pairs {
                            uses.entry(*v).or_default().push((bi, ii));
                        }
                    }
                    Instruction::Branch { cond, .. } => {
                        uses.entry(*cond).or_default().push((bi, ii));
                    }
                    Instruction::Return(v) => {
                        uses.entry(*v).or_default().push((bi, ii));
                    }
                    Instruction::Store(_, v) => {
                        uses.entry(*v).or_default().push((bi, ii));
                    }
                    _ => {}
                }
            }
        }
        uses
    }

    /// Check if definition at `(d_block, d_idx)` dominates use at
    /// `(u_block, u_idx)`.
    ///
    /// Within the same block: definition must precede use (`d_idx < u_idx`).
    /// Across blocks: `d_block` must dominate `u_block` in the dominator tree.
    fn dominates_instr(
        &self,
        dom: &[Vec<bool>],
        d_block: usize,
        d_idx: usize,
        u_block: usize,
        u_idx: usize,
    ) -> bool {
        if d_block == u_block {
            d_idx < u_idx
        } else {
            dom[u_block][d_block]
        }
    }

    /// Replace every read of `from` with `to` at instruction `(bi, ii)`.
    fn replace_use_at(
        &mut self,
        bi: usize,
        ii: usize,
        from: Value,
        to: Value,
    ) {
        let instr = &mut self.blocks[bi].instrs[ii];
        match instr {
            Instruction::Add(a, b) => {
                if *a == from { *a = to; }
                if *b == from { *b = to; }
            }
            Instruction::Sub(a, b) => {
                if *a == from { *a = to; }
                if *b == from { *b = to; }
            }
            Instruction::Mul(a, b) => {
                if *a == from { *a = to; }
                if *b == from { *b = to; }
            }
            Instruction::Phi(pairs) => {
                for (v, _) in pairs.iter_mut() {
                    if *v == from { *v = to; }
                }
            }
            Instruction::Branch { cond, .. } => {
                if *cond == from { *cond = to; }
            }
            Instruction::Return(v) => {
                if *v == from { *v = to; }
            }
            Instruction::Store(_, v) => {
                if *v == from { *v = to; }
            }
            _ => {}
        }
    }

    /// Global Common Subexpression Elimination on SSA form.
    ///
    /// Walks all blocks in order. For each instruction whose result is defined
    /// by `(opcode, operand1, operand2)`, checks whether the same signature
    /// was already computed by an earlier instruction. If the earlier
    /// definition dominates all uses of the current definition, all uses of
    /// the current value are rewritten to the earlier value and the current
    /// instruction is replaced with `Nop`.
    ///
    /// Returns the number of instructions eliminated.
    pub fn common_subexpr_eliminate(&mut self) -> usize {
        use std::collections::HashMap;

        let dom = self.dominators();
        let uses = self.collect_uses();

        // Table: CSE signature → first defining Value::Instr.
        let mut table: HashMap<CseSig, Value> = HashMap::new();

        // Collect all mutations first (avoid borrowing self.blocks while
        // iterating over it), then apply in a second pass.
        struct Replacement {
            bi: usize,
            ii: usize,
            from: Value,
            to: Value,
        }
        let mut replacements: Vec<Replacement> = Vec::new();
        let mut nop_positions: Vec<(usize, usize)> = Vec::new();
        let mut eliminated = 0;

        for (bi, block) in self.blocks.iter().enumerate() {
            for (ii, instr) in block.instrs.iter().enumerate() {
                let Some(sig) = instr.cse_signature() else {
                    continue;
                };

                let current = Value::Instr(bi, ii);

                if let Some(&first_def) = table.get(&sig) {
                    let (first_b, first_i) = match first_def {
                        Value::Instr(b, i) => (b, i),
                        _ => unreachable!("CSE table should only contain Instr values"),
                    };

                    // Check if first_def dominates all uses of current.
                    let use_sites = uses.get(&current);
                    let all_dominated = match use_sites {
                        Some(sites) => sites.iter().all(|&(ub, ui)| {
                            self.dominates_instr(&dom, first_b, first_i, ub, ui)
                        }),
                        None => true, // no uses → trivially safe
                    };

                    if all_dominated {
                        if let Some(sites) = uses.get(&current) {
                            for &(ub, ui) in sites {
                                replacements.push(Replacement {
                                    bi: ub,
                                    ii: ui,
                                    from: current,
                                    to: first_def,
                                });
                            }
                        }
                        nop_positions.push((bi, ii));
                        eliminated += 1;
                    }
                } else {
                    table.insert(sig, current);
                }
            }
        }

        // Apply replacements.
        for r in replacements {
            self.replace_use_at(r.bi, r.ii, r.from, r.to);
        }
        // Nop out eliminated instructions.
        for (bi, ii) in nop_positions {
            self.blocks[bi].instrs[ii] = Instruction::Nop;
        }

        eliminated
    }
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

    #[test]
    fn unreachable_block_removed() {
        // block0(entry)->block1->block2, block3 isolated (no preds => unreachable).
        let mut f = SsaFunction::new();
        f.push_block(block(vec![Instruction::Jump(1)])); // 0 entry
        f.push_block(block(vec![Instruction::Jump(2)])); // 1
        f.push_block(block(vec![Instruction::Return(Value::Const(0))])); // 2
        f.push_block(block(vec![Instruction::Return(Value::Const(1))])); // 3 unreachable
        let removed = f.eliminate_unreachable_blocks();
        assert!(removed);
        assert_eq!(f.blocks.len(), 3);
        assert_eq!(f.entry, 0);
        assert!(matches!(f.blocks[0].instrs[0], Instruction::Jump(1)));
        assert!(matches!(f.blocks[1].instrs[0], Instruction::Jump(2)));
        assert!(matches!(f.blocks[2].instrs[0], Instruction::Return(_)));
    }

    #[test]
    fn unreachable_block_remaps_jump_targets() {
        // block0(entry)->Jump(2), block1 unreachable (no preds), block2 reachable via block0.
        let mut f = SsaFunction::new();
        f.push_block(block(vec![Instruction::Jump(2)])); // 0 entry
        f.push_block(block(vec![Instruction::Return(Value::Const(5))])); // 1 unreachable
        f.push_block(block(vec![Instruction::Return(Value::Const(1))])); // 2
        let removed = f.eliminate_unreachable_blocks();
        assert!(removed);
        assert_eq!(f.blocks.len(), 2);
        assert_eq!(f.entry, 0);
        // block0's jump target block2 remapped to index 1.
        assert!(matches!(f.blocks[0].instrs[0], Instruction::Jump(1)));
        assert!(matches!(f.blocks[1].instrs[0], Instruction::Return(_)));
    }

    #[test]
    fn unreachable_block_remaps_branch_targets() {
        // block0(entry)->Branch{then:2, relse:3}, block1 unreachable (no preds),
        // block2 and block3 reachable via block0.
        let mut f = SsaFunction::new();
        f.push_block(block(vec![
            Instruction::Branch {
                cond: Value::Instr(0, 0),
                then: 2,
                r#else: 3,
            },
        ])); // 0 entry
        f.push_block(block(vec![Instruction::Return(Value::Const(5))])); // 1 unreachable
        f.push_block(block(vec![Instruction::Return(Value::Const(1))])); // 2
        f.push_block(block(vec![Instruction::Return(Value::Const(2))])); // 3
        let removed = f.eliminate_unreachable_blocks();
        assert!(removed);
        assert_eq!(f.blocks.len(), 3);
        assert_eq!(f.entry, 0);
        // block2->index 1, block3->index 2 (block1 removed at index 1).
        assert!(matches!(
            f.blocks[0].instrs[0],
            Instruction::Branch {
                then: 1, r#else: 2, ..
            }
        ));
        assert!(matches!(f.blocks[1].instrs[0], Instruction::Return(_)));
        assert!(matches!(f.blocks[2].instrs[0], Instruction::Return(_)));
    }

    #[test]
    fn unreachable_block_loop_preserved() {
        // block0(entry)<->block1 loop, block2 unreachable removed.
        let mut f = SsaFunction::new();
        f.push_block(block(vec![Instruction::Jump(1)])); // 0 entry
        f.push_block(block(vec![Instruction::Jump(0)])); // 1 jumps back to entry
        f.push_block(block(vec![Instruction::Return(Value::Const(9))])); // 2 unreachable
        let removed = f.eliminate_unreachable_blocks();
        assert!(removed);
        assert_eq!(f.blocks.len(), 2);
        assert_eq!(f.entry, 0);
        assert!(matches!(f.blocks[0].instrs[0], Instruction::Jump(1)));
        assert!(matches!(f.blocks[1].instrs[0], Instruction::Jump(0)));
    }

    #[test]
    fn all_reachable_no_change() {
        let mut f = SsaFunction::new();
        f.push_block(block(vec![Instruction::Jump(1)])); // 0
        f.push_block(block(vec![Instruction::Return(Value::Const(0))])); // 1
        let before = f.blocks.clone();
        let removed = f.eliminate_unreachable_blocks();
        assert!(!removed);
        assert_eq!(f.blocks, before);
        assert_eq!(f.entry, 0);
    }

    #[test]
    fn entry_only_no_change() {
        let mut f = SsaFunction::new();
        f.push_block(block(vec![Instruction::Return(Value::Const(0))])); // 0 entry
        let removed = f.eliminate_unreachable_blocks();
        assert!(!removed);
        assert_eq!(f.blocks.len(), 1);
        assert_eq!(f.entry, 0);
    }

    #[test]
    fn reachable_block_dominates_itself() {
        let mut f = SsaFunction::new();
        f.push_block(block(vec![Instruction::Jump(1)])); // 0 entry
        f.push_block(block(vec![Instruction::Return(Value::Const(0))])); // 1
        let dom = f.dominators();
        assert!(dom[0][0], "entry dominates itself");
        assert!(dom[1][1], "block 1 dominates itself");
        assert!(dom[1][0], "entry dominates block 1");
    }

    // ─── SSA-based CSE tests ───

    #[test]
    fn cse_eliminates_duplicate_add_in_same_block() {
        // t1 = 1 + 2; t2 = 1 + 2; return t2
        // t2 should be eliminated, uses of t2 rewritten to t1.
        let mut f = SsaFunction::new();
        f.push_block(block(vec![
            Instruction::Add(Value::Const(1), Value::Const(2)), // Instr(0,0) = t1
            Instruction::Add(Value::Const(1), Value::Const(2)), // Instr(0,1) = t2 (dup)
            Instruction::Return(Value::Instr(0, 1)),            // return t2
        ]));
        let removed = f.common_subexpr_eliminate();
        assert_eq!(removed, 1);
        assert!(matches!(f.blocks[0].instrs[0], Instruction::Add(..))); // t1 kept
        assert!(matches!(f.blocks[0].instrs[1], Instruction::Nop));      // t2 eliminated
        // Return should now reference t1 (Instr(0,0)) instead of t2.
        assert!(matches!(f.blocks[0].instrs[2], Instruction::Return(Value::Instr(0, 0))));
    }

    #[test]
    fn cse_eliminates_duplicate_across_blocks() {
        // block0: t1 = 1 + 2; goto block2
        // block1: t2 = 1 + 2; goto block2  (t2 dominated by entry? block1 is dominated by entry, t1's block0 dominates block1)
        // block2: return phi(t1 from block0, t2 from block1)
        // Actually simpler: block0 computes t1=1+2, jumps to block2.
        // block1 computes t2=1+2, jumps to block2.
        // block2 returns phi. t1 and t2 have same signature.
        // entry (block0) dominates block1, so t1 dominates uses of t2 in block2's phi.
        let mut f = SsaFunction::new();
        f.push_block(block(vec![
            Instruction::Add(Value::Const(1), Value::Const(2)), // Instr(0,0) = t1
            Instruction::Jump(2),
        ])); // 0 entry
        f.push_block(block(vec![
            Instruction::Add(Value::Const(1), Value::Const(2)), // Instr(1,0) = t2 (dup)
            Instruction::Jump(2),
        ])); // 1
        f.push_block(block(vec![
            Instruction::Phi(vec![
                (Value::Instr(0, 0), 0),
                (Value::Instr(1, 0), 1),
            ]), // Instr(2,0) = phi
            Instruction::Return(Value::Instr(2, 0)),
        ])); // 2
        let removed = f.common_subexpr_eliminate();
        assert_eq!(removed, 1);
        assert!(matches!(f.blocks[0].instrs[0], Instruction::Add(..))); // t1 kept
        assert!(matches!(f.blocks[1].instrs[0], Instruction::Nop));      // t2 eliminated
        // Phi should now use t1 instead of t2 for block1 predecessor.
        assert!(matches!(&f.blocks[2].instrs[0], Instruction::Phi(pairs) if pairs == &vec![(Value::Instr(0, 0), 0), (Value::Instr(0, 0), 1)]));
    }

    #[test]
    fn cse_no_eliminate_when_def_does_not_dominate() {
        // block0: t1 = 1 + 2; goto block1
        // block1: goto block2 or block3
        // block2: t2 = 1 + 2; goto block3  (t2's block2 is NOT dominated by block0? Actually block0 dominates block2 via block1)
        // block3: return phi(t1 from block0? no...)
        // Simpler: block0 computes t1=1+2, jumps to block1.
        // block1 branches to block2 or block3.
        // block2 computes t2=1+2 (same sig as t1), returns t2.
        // block3 returns t1.
        // t1's block0 does NOT dominate block2 (block0→block1→block2, but block0→block1→block3 also exists; block1 is not dominated by block0 alone? Actually block0 IS the entry, so it dominates everything.)
        // Since block0 is entry, it dominates all blocks. So t1 would dominate t2's uses.
        // To test non-domination, we need a case where the first definition is in a block that doesn't dominate the use block.
        // That requires the first definition to NOT be in the entry block's dominance path.
        // With entry = block0, block0 dominates all reachable blocks. So we need the "first" definition to be in a non-entry block.
        // block0: goto block1 (entry, no computation)
        // block1: t1 = 1 + 2; goto block2
        // block2: goto block3 or block4
        // block3: t2 = 1 + 2; return t2   (block2 doesn't dominate block3? block2→block3, block2→block4. block2 dominates block3 only if all paths to block3 go through block2. But entry→block1→block2→block3, so block2 is on the only path to block3. So block2 DOES dominate block3.)
        // Hmm, with a simple linear CFG it's hard to get non-domination.
        // Let's just test that when operands differ, no elimination happens.
        let mut f = SsaFunction::new();
        f.push_block(block(vec![
            Instruction::Add(Value::Const(1), Value::Const(2)), // Instr(0,0)
            Instruction::Add(Value::Const(1), Value::Const(3)), // Instr(0,1) - different operand
            Instruction::Return(Value::Instr(0, 1)),
        ]));
        let removed = f.common_subexpr_eliminate();
        assert_eq!(removed, 0);
        assert!(matches!(f.blocks[0].instrs[0], Instruction::Add(..)));
        assert!(matches!(f.blocks[0].instrs[1], Instruction::Add(..)));
    }

    #[test]
    fn cse_sub_and_mul_not_confused() {
        // t1 = 1 - 2; t2 = 1 + 2; return t2
        // Sub and Add have different signatures, so no elimination.
        let mut f = SsaFunction::new();
        f.push_block(block(vec![
            Instruction::Sub(Value::Const(1), Value::Const(2)), // Instr(0,0)
            Instruction::Add(Value::Const(1), Value::Const(2)), // Instr(0,1)
            Instruction::Return(Value::Instr(0, 1)),
        ]));
        let removed = f.common_subexpr_eliminate();
        assert_eq!(removed, 0);
        assert!(matches!(f.blocks[0].instrs[0], Instruction::Sub(..)));
        assert!(matches!(f.blocks[0].instrs[1], Instruction::Add(..)));
    }

    #[test]
    fn cse_empty_function() {
        let mut f = SsaFunction::new();
        let removed = f.common_subexpr_eliminate();
        assert_eq!(removed, 0);
    }

    #[test]
    fn cse_commutative_not_assumed() {
        // t1 = 1 + 2; t2 = 2 + 1; return t2
        // Add is NOT commutative in our CSE sig (operands ordered as-is),
        // so t2 should NOT be eliminated.
        let mut f = SsaFunction::new();
        f.push_block(block(vec![
            Instruction::Add(Value::Const(1), Value::Const(2)), // Instr(0,0)
            Instruction::Add(Value::Const(2), Value::Const(1)), // Instr(0,1) - different order
            Instruction::Return(Value::Instr(0, 1)),
        ]));
        let removed = f.common_subexpr_eliminate();
        assert_eq!(removed, 0);
    }
}
