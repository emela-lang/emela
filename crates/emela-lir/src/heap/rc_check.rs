//! The reference counting balance check of the verifier (Heap IR at [`crate::Stage::RefCounted`]).
//!
//! Follows every path of a function, counting the references each `ref` variable owns under the
//! ownership rules of [`super::rc`]: a definition gives one (none for views and the borrowed
//! environment), `dup` adds one, `drop` and each consuming use take one. It reports a reference
//! taken that is not there, a borrowed use of a variable without a reference, references left when
//! leaving the function or a scope, and edges into a join point (the end of a compound statement,
//! a loop head) that bring different references.

use rustc_hash::FxHashMap;

use super::rc::{Mode, borrowed_param, counted, is_view, landings, term_consumes, value_uses};
use crate::ir::{Function, LabelId, Module, SeqId, Stmt, SwitchOn, Term, VarId};

/// The messages of the imbalances found in the function.
pub(crate) fn check(m: &Module, f: &Function) -> Vec<String> {
    let mut c = Check {
        m,
        f,
        scope: Vec::new(),
        slots: Vec::new(),
        labels: FxHashMap::default(),
        errors: Vec::new(),
    };
    let Some(body) = f.body else {
        return Vec::new();
    };
    let mut tokens = vec![0; f.vars.len()];
    let params: Vec<(VarId, i32)> = f
        .params
        .iter()
        .filter(|&&p| counted(f, p))
        .map(|&p| (p, i32::from(!borrowed_param(f, p))))
        .collect();
    for &(p, n) in &params {
        tokens[ix(p)] = n;
        c.scope.push(p);
    }
    c.seq(body, tokens, None, None);
    c.errors
}

fn ix<T>(id: la_arena::Idx<T>) -> usize {
    u32::from(id.into_raw()) as usize
}

/// References per variable, by index.
type Tokens = Vec<i32>;

/// A join point: the end of a compound statement or a loop head.
struct Slot {
    /// The length of the scope at the statement: variables before it are outer.
    mark: usize,
    /// The references of the outer variables, set by the first edge (or the loop entry).
    expected: Option<Vec<i32>>,
}

struct Check<'a> {
    m: &'a Module,
    f: &'a Function,
    /// Counted variables in scope, in definition order.
    scope: Vec<VarId>,
    slots: Vec<Slot>,
    /// The slot of each block.
    labels: FxHashMap<LabelId, usize>,
    errors: Vec<String>,
}

impl Check<'_> {
    fn name(&self, v: VarId) -> String {
        format!("{}%{}", self.f.vars[v].hint, ix(v))
    }

    fn error(&mut self, message: String) {
        self.errors.push(message);
    }

    fn define(&mut self, tokens: &mut Tokens, v: VarId, n: i32) {
        if counted(self.f, v) {
            tokens[ix(v)] = n;
            self.scope.push(v);
        }
    }

    fn take(&mut self, tokens: &mut Tokens, v: VarId, what: &str) {
        tokens[ix(v)] -= 1;
        if tokens[ix(v)] < 0 {
            let name = self.name(v);
            self.error(format!("{what} {name}, which owns no reference"));
            tokens[ix(v)] = 0;
        }
    }

    fn borrow(&mut self, tokens: &Tokens, v: VarId) {
        if tokens[ix(v)] < 1 && !borrowed_param(self.f, v) {
            let name = self.name(v);
            self.error(format!(
                "{name} is used without a reference (after its last one)"
            ));
        }
    }

    /// Leaves to the slot (or out of the function with `None`) with the state.
    fn deliver(&mut self, slot: Option<usize>, tokens: &Tokens) {
        let mark = slot.map_or(0, |s| self.slots[s].mark);
        for i in mark..self.scope.len() {
            let v = self.scope[i];
            if tokens[ix(v)] != 0 {
                let name = self.name(v);
                let n = tokens[ix(v)];
                self.error(format!("{name} leaves its scope with {n} references"));
            }
        }
        let Some(s) = slot else {
            return;
        };
        let outer: Vec<i32> = self.scope[..mark].iter().map(|v| tokens[ix(*v)]).collect();
        match &self.slots[s].expected {
            None => self.slots[s].expected = Some(outer),
            Some(expected) => {
                let differ: Vec<VarId> = self.scope[..mark]
                    .iter()
                    .zip(expected.iter().zip(&outer))
                    .filter(|(_, (a, b))| a != b)
                    .map(|(v, _)| *v)
                    .collect();
                for v in differ {
                    let name = self.name(v);
                    self.error(format!(
                        "{name} has different references on the edges into a join point"
                    ));
                }
            }
        }
    }

    /// The state after a join point, or `None` if no edge reaches it.
    fn joined(&self, slot: usize, mut tokens: Tokens) -> Option<Tokens> {
        let s = &self.slots[slot];
        let expected = s.expected.as_ref()?;
        for (v, n) in self.scope[..s.mark].iter().zip(expected) {
            tokens[ix(*v)] = *n;
        }
        Some(tokens)
    }

    fn new_slot(&mut self) -> usize {
        self.slots.push(Slot {
            mark: self.scope.len(),
            expected: None,
        });
        self.slots.len() - 1
    }

    /// Checks the sequence from the state. `yield_to` and `head` are the slots of `yield` and
    /// `recur`.
    fn seq(&mut self, id: SeqId, tokens: Tokens, yield_to: Option<usize>, head: Option<usize>) {
        self.seq_with(id, tokens, &[], yield_to, head);
    }

    /// [`Self::seq`] with variables defined at its start.
    fn seq_with(
        &mut self,
        id: SeqId,
        mut tokens: Tokens,
        defs: &[(VarId, i32)],
        yield_to: Option<usize>,
        head: Option<usize>,
    ) {
        let mark = self.scope.len();
        for &(v, n) in defs {
            self.define(&mut tokens, v, n);
        }
        let f = self.f;
        let seq = &f.seqs[id];
        let mut state = Some(tokens);
        for &s in &seq.stmts {
            let Some(tokens) = state else {
                break;
            };
            state = self.stmt(s, tokens, head);
        }
        if let Some(mut tokens) = state {
            let consumed = term_consumes(f, &seq.term);
            for v in consumed {
                self.take(&mut tokens, v, "the terminator consumes");
            }
            match &seq.term {
                Term::Yield(_) => self.deliver(yield_to, &tokens),
                Term::Break { label, .. } => {
                    let slot = self.labels.get(label).copied();
                    if slot.is_some() {
                        self.deliver(slot, &tokens);
                    }
                }
                Term::Recur(_) => {
                    if head.is_some() {
                        self.deliver(head, &tokens);
                    }
                }
                Term::Return(_) | Term::Fail(_) | Term::Unwind | Term::Defect(_) => {
                    self.deliver(None, &tokens)
                }
                Term::Unreachable => {}
            }
        }
        self.scope.truncate(mark);
    }

    fn stmt(
        &mut self,
        s: crate::ir::StmtId,
        mut tokens: Tokens,
        head: Option<usize>,
    ) -> Option<Tokens> {
        let f = self.f;
        let stmt = &f.stmts[s];
        match stmt {
            Stmt::Let { dst, value, .. } => {
                let mut uses = Vec::new();
                value_uses(self.m, f, value, &mut |v, mode| uses.push((v, mode)));
                for &(v, mode) in &uses {
                    if mode == Mode::Consume {
                        self.take(&mut tokens, v, "a consuming use of");
                    }
                }
                for &(v, mode) in &uses {
                    if mode == Mode::Borrow {
                        self.borrow(&tokens, v);
                    }
                }
                let (fail, defect) = landings(stmt);
                if let Some((e, body)) = fail {
                    self.seq_with(body, tokens.clone(), &[(e, 1)], None, head);
                }
                if let Some(l) = defect {
                    self.seq(l, tokens.clone(), None, head);
                }
                if let Some(d) = dst {
                    let n = i32::from(!is_view(f, value));
                    self.define(&mut tokens, *d, n);
                }
                Some(tokens)
            }
            Stmt::Dup(v) => {
                tokens[ix(*v)] += 1;
                Some(tokens)
            }
            Stmt::Drop(v) => {
                self.take(&mut tokens, *v, "drop of");
                Some(tokens)
            }
            Stmt::If {
                dst, then, else_, ..
            } => {
                let slot = self.new_slot();
                self.seq(*then, tokens.clone(), Some(slot), head);
                self.seq(*else_, tokens.clone(), Some(slot), head);
                self.after(slot, tokens, *dst)
            }
            Stmt::Switch {
                dst,
                on,
                cases,
                default,
            } => {
                let (SwitchOn::Tag(v) | SwitchOn::Lit(v)) = on;
                if counted(f, *v) {
                    self.borrow(&tokens, *v);
                }
                let slot = self.new_slot();
                for c in cases {
                    self.seq(c.body, tokens.clone(), Some(slot), head);
                }
                if let Some(d) = default {
                    self.seq(*d, tokens.clone(), Some(slot), head);
                }
                self.after(slot, tokens, *dst)
            }
            Stmt::Block { dst, label, body } => {
                let slot = self.new_slot();
                self.labels.insert(*label, slot);
                self.seq(*body, tokens.clone(), Some(slot), head);
                self.after(slot, tokens, *dst)
            }
            Stmt::Loop {
                dst,
                vars,
                inits,
                body,
            } => {
                let consumed = super::rc::operand_vars(f, &inits.iter().collect::<Vec<_>>());
                for v in consumed {
                    self.take(&mut tokens, v, "a loop's initial value consumes");
                }
                let exit = self.new_slot();
                let at_head = self.new_slot();
                let outer: Vec<i32> = self.scope.iter().map(|v| tokens[ix(*v)]).collect();
                self.slots[at_head].expected = Some(outer);
                let defs: Vec<(VarId, i32)> = vars.iter().map(|&v| (v, 1)).collect();
                self.seq_with(*body, tokens.clone(), &defs, Some(exit), Some(at_head));
                self.after(exit, tokens, *dst)
            }
            Stmt::Protect { .. } => {
                self.error("protect in reference-counted code".into());
                None
            }
        }
    }

    /// The state after a compound statement: the joined one, with its destination defined.
    fn after(&mut self, slot: usize, tokens: Tokens, dst: Option<VarId>) -> Option<Tokens> {
        let mut tokens = self.joined(slot, tokens)?;
        if let Some(d) = dst {
            self.define(&mut tokens, d, 1);
        }
        Some(tokens)
    }
}
