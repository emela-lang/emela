//! The IR lowered to instructions: sequences, terminators, statements and switches, the fields,
//! constructors and reuse of a `let`'s value, primitive operations, calls and the fiber
//! operations. They call each other as the IR nests, so they are one module; `Builder::value`,
//! which the state machines of `suspend` lower values with too, stays in the parent.

use super::*;

/// Below this many cases, the chain of comparing `if`s is shorter than the nested blocks and the
/// dispatch of a table.
pub(super) const SWITCH_TABLE_MIN_CASES: usize = 3;

/// How many table entries (bytes) a case of the chain may be worth. A chain case is a comparison
/// and its `if`, which is more than one byte, so the table may be this much wider than the switch
/// has cases.
pub(super) const SWITCH_TABLE_WIDTH_SCALE: usize = 8;

/// A switch's scrutinee.
#[derive(Clone, Copy)]
pub(super) enum Scrutinee {
    /// An enum value with a heap representation: the tag is read with the `tag` runtime function.
    Tag(VarId),
    /// An enum value whose representation is its variant number.
    Variant(VarId),
    /// An enum value held by value in locals: its one constructor is 0.
    TagZero,
    /// A literal `i32` or `i64` value.
    Lit(VarId),
}

impl Builder<'_> {
    pub(super) fn seq(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        id: SeqId,
        yield_dst: Option<VarId>,
    ) {
        let stmts = cx.f.seqs[id].stmts.clone();
        for s in stmts {
            self.stmt(cx, ins, s);
        }
        let term = cx.f.seqs[id].term.clone();
        self.term(cx, ins, &term, yield_dst);
    }

    pub(super) fn term(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        term: &Term,
        yield_dst: Option<VarId>,
    ) {
        match term {
            Term::Yield(v) => {
                if let (Some(d), Some(v)) = (yield_dst, v) {
                    self.operand(cx, ins, v);
                    let repr = Self::operand_repr(cx.f, v);
                    self.assign(cx, ins, Some(d), repr);
                } else if let (None, Some(_)) = (yield_dst, v) {
                    self.internal(format!("yield without a target in {}", cx.f.name));
                }
            }
            Term::Return(v) => {
                // A flattened function's result is one wasm result per leaf, so the operand must
                // push exactly as many values as the signature declares.
                let width = v.as_ref().map_or(0, |o| Self::operand_width(cx, o));
                if width != cx.ret_leaves.len() {
                    self.internal(format!(
                        "return of {width} values from {} with {} results",
                        cx.f.name,
                        cx.ret_leaves.len()
                    ));
                    return;
                }
                if let Some(v) = v {
                    self.operand(cx, ins, v);
                }
                ins.return_();
            }
            Term::Fail(v) => {
                self.operand(cx, ins, v);
                ins.global_set(G_FAILURE);
                dummy_return_values(ins, &cx.ret_leaves);
            }
            Term::Break { label, value } => {
                let Some((depth, dst)) = cx.find_block(*label) else {
                    self.internal(format!("break to an unknown label in {}", cx.f.name));
                    return;
                };
                if let (Some(d), Some(v)) = (dst, value) {
                    self.operand(cx, ins, v);
                    let repr = Self::operand_repr(cx.f, v);
                    self.assign(cx, ins, Some(d), repr);
                }
                ins.br(depth);
            }
            Term::Recur(args) => {
                let Some((depth, vars)) = cx.find_loop() else {
                    self.internal(format!("recur outside a loop in {}", cx.f.name));
                    return;
                };
                // Parallel assignment through scratch locals. A value held by value contributes
                // one scratch slot per leaf, in field order; `recur_slots` made room for the
                // widest `recur`.
                let mut slot = 0u32;
                for (o, &v) in args.iter().zip(&vars) {
                    self.operand(cx, ins, o);
                    if let Some(n) = cx.aggregate(v) {
                        for i in (0..n).rev() {
                            let r = cx.leaf_repr(v, i);
                            ins.local_set(cx.scratch(r, (slot + i) as usize));
                        }
                        slot += n;
                    } else {
                        let r = cx.f.vars[v].repr;
                        ins.local_set(cx.scratch(r, slot as usize));
                        slot += 1;
                    }
                }
                let mut slot = 0u32;
                for &v in &vars {
                    if let Some(n) = cx.aggregate(v) {
                        for i in 0..n {
                            let r = cx.leaf_repr(v, i);
                            ins.local_get(cx.scratch(r, (slot + i) as usize));
                            ins.local_set(cx.leaf(v, i));
                        }
                        slot += n;
                    } else {
                        let r = cx.f.vars[v].repr;
                        ins.local_get(cx.scratch(r, slot as usize));
                        ins.local_set(cx.local(v));
                        slot += 1;
                    }
                }
                ins.br(depth);
            }
            Term::Unwind => dummy_return_values(ins, &cx.ret_leaves),
            Term::Unreachable => {
                ins.unreachable();
            }
            Term::Defect(..) => self.internal(format!("defect left in {}", cx.f.name)),
        }
    }

    pub(super) fn stmt(&mut self, cx: &mut FnCtx, ins: &mut InstructionSink, id: StmtId) {
        let stmt = cx.f.stmts[id].clone();
        match stmt {
            Stmt::Let {
                dst,
                value,
                on_defect,
                on_suspend,
            } => {
                if on_suspend.is_some() {
                    // State machines have their own emitter (see `suspend`).
                    self.internal(format!("suspension edge left in {}", cx.f.name));
                }
                // Suspending code called where nothing may suspend runs in place (spec 8.7);
                // what comes back suspended is unwound as interrupted (spec 14.5).
                let sync = !cx.f.suspends && emela_lir::may_suspend(&value);
                if sync {
                    ins.call(self.rt("sync_enter"));
                }
                match cx.f.reuse.get(&id).copied() {
                    // Perceus reuse (C4/PR-042): write the new fields into the old block instead
                    // of allocating a new one.
                    Some(old) => self.reuse_make(cx, ins, dst, old, &value),
                    None => self.value(cx, ins, dst, &value),
                }
                if sync {
                    ins.call(self.rt("sync_leave"));
                }
                if let Some(landing) = on_defect {
                    ins.global_get(G_DEFECT);
                    ins.if_(BlockType::Empty);
                    cx.push(Lbl::Other);
                    self.seq(cx, ins, landing, None);
                    cx.pop();
                    ins.end();
                }
                if let Some(on_fail) = value_on_fail(&value) {
                    match on_fail {
                        OnFail::Handle { error, body } => {
                            ins.global_get(G_FAILURE);
                            ins.if_(BlockType::Empty);
                            cx.push(Lbl::Other);
                            ins.global_get(G_FAILURE);
                            ins.local_set(cx.local(*error));
                            ins.i32_const(0);
                            ins.global_set(G_FAILURE);
                            self.seq(cx, ins, *body, None);
                            cx.pop();
                            ins.end();
                        }
                        _ => self.internal(format!("Flow IR failure edge in {}", cx.f.name)),
                    }
                }
            }
            Stmt::If {
                dst,
                cond,
                then,
                else_,
            } => {
                self.operand(cx, ins, &cond);
                ins.if_(BlockType::Empty);
                cx.push(Lbl::Other);
                self.seq(cx, ins, then, dst);
                ins.else_();
                self.seq(cx, ins, else_, dst);
                cx.pop();
                ins.end();
            }
            Stmt::Switch {
                dst,
                on,
                cases,
                default,
            } => {
                let scrutinee = match on {
                    SwitchOn::Tag(v) => {
                        if cx.aggregate(v).is_some() {
                            // A value held by value has exactly one constructor, so its tag is 0
                            // (as the C backend does).
                            Scrutinee::TagZero
                        } else if cx.f.vars[v].repr == Repr::Ref {
                            Scrutinee::Tag(v)
                        } else {
                            Scrutinee::Variant(v)
                        }
                    }
                    SwitchOn::Lit(v) => Scrutinee::Lit(v),
                };
                self.switch(cx, ins, &cases, &default, dst, scrutinee);
            }
            Stmt::Loop {
                dst,
                vars,
                inits,
                body,
            } => {
                for (&v, init) in vars.iter().zip(&inits) {
                    self.operand(cx, ins, init);
                    let repr = Self::operand_repr(cx.f, init);
                    self.assign(cx, ins, Some(v), repr);
                }
                ins.loop_(BlockType::Empty);
                cx.push(Lbl::Loop(vars.clone()));
                self.seq(cx, ins, body, dst);
                cx.pop();
                ins.end();
            }
            Stmt::Block { dst, label, body } => {
                ins.block(BlockType::Empty);
                cx.push(Lbl::Block(label, dst));
                self.seq(cx, ins, body, dst);
                cx.pop();
                ins.end();
            }
            Stmt::Protect { .. } => self.internal(format!("protect left in {}", cx.f.name)),
            Stmt::Dup(v) => {
                // A value held by value owns no reference: nothing to count (the C backend's
                // `count` does the same).
                if cx.aggregate(v).is_some() {
                    return;
                }
                ins.local_get(cx.local(v));
                // `dup`: count the object unless it is null or static (rc == 0).
                let obj = cx.scratch(Repr::I32, 0);
                let rc = cx.scratch(Repr::Ref, 0);
                ins.local_set(obj);
                ins.local_get(obj);
                ins.i32_eqz();
                ins.if_(BlockType::Empty);
                ins.else_();
                ins.local_get(obj);
                ins.i32_load(mem(0, 4));
                ins.local_set(rc);
                ins.local_get(rc);
                ins.i32_eqz();
                ins.if_(BlockType::Empty);
                ins.else_();
                ins.local_get(obj);
                ins.local_get(rc);
                ins.i32_const(1);
                ins.i32_add();
                ins.i32_store(mem(0, 4));
                ins.end();
                ins.end();
            }
            Stmt::Drop(v) => {
                // A value held by value owns no reference: nothing to release.
                if cx.aggregate(v).is_some() {
                    return;
                }
                ins.local_get(cx.local(v));
                // Drop specialisation (C3/PR-027): a value whose payload holds no reference and
                // whose type has no finalizer gives its block back without walking it.
                let name = match cx.f.drop_kinds.get(&id) {
                    Some(emela_lir::opt::drop_spec::DropKind::Block) => "drop_block",
                    _ => "drop",
                };
                let drop = self.rt(name);
                ins.call(drop);
            }
        }
    }

    /// A switch on an enum tag whose variant keys are dense enough is dispatched with `br_table`;
    /// everything else (a sparse table, a switch on a literal, too few cases) stays the chain of
    /// comparing `if`s. A literal switch cannot use a table: its keys are arbitrary `i32`s, and
    /// an `i64` one is not the `i32` a `br_table` indexes with.
    ///
    /// The keys and their span come from the LIR ([`emela_lir::opt::switch::variant_keys`],
    /// [`emela_lir::opt::switch::variant_span`]); when the normalisation pass ran they are already
    /// sorted, the keys of a shared body are joined and duplicates are gone, and the emitter only
    /// decides whether a table is worth its bytes. The helpers also accept the un-normalised form
    /// of a `--no-opt` build.
    pub(super) fn switch(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        cases: &[emela_lir::Case],
        default: &Option<SeqId>,
        dst: Option<VarId>,
        scrutinee: Scrutinee,
    ) {
        if matches!(scrutinee, Scrutinee::Lit(_)) {
            return self.switch_chain(cx, ins, cases, 0, default, dst, scrutinee);
        }
        // The verifier gives a tag switch only variant keys, in range of the enum, each once.
        let Some(keys) = emela_lir::opt::switch::variant_keys(cases) else {
            self.internal(format!("a literal case in a tag switch in {}", cx.f.name));
            return;
        };
        let Some(width) = emela_lir::opt::switch::variant_span(cases) else {
            return self.switch_chain(cx, ins, cases, 0, default, dst, scrutinee);
        };
        // A table entry costs a byte; a case of the chain costs a comparison and its `if`, which
        // is more. A table wider than that is not worth it, nor is one for a couple of cases,
        // where the dispatch is one more label than the comparison it replaces. The count is of
        // keys, not of bodies: a chain compares every key, a table has one block per body.
        if keys.len() < SWITCH_TABLE_MIN_CASES || width > SWITCH_TABLE_WIDTH_SCALE * keys.len() {
            return self.switch_chain(cx, ins, cases, 0, default, dst, scrutinee);
        }
        self.switch_table(cx, ins, cases, default, dst, scrutinee, width);
    }

    /// A switch as one nested block per case, dispatched by `br_table`, with a block around them
    /// for the end of the switch and a block inside it for the default.
    ///
    /// The blocks are pushed in case order, so the innermost (last pushed) block belongs to the
    /// first case: a `br_table` depth of `i` leaves the blocks of the cases before `i` and lands
    /// after the `end` of case `i`'s block, which is where case `i`'s body starts. A gap between
    /// variant numbers lands on the default block. A case body that falls through (`yield`) leaves
    /// the switch at the outer block, past the default, so no case can run the default's code.
    /// Every one of those blocks is on `cx.labels`, so the depth of a break or a recur out of a
    /// case body counts them exactly as any other `if` or `block` would be counted.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn switch_table(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        cases: &[emela_lir::Case],
        default: &Option<SeqId>,
        dst: Option<VarId>,
        scrutinee: Scrutinee,
        width: usize,
    ) {
        let n = cases.len();
        // The outer block is the switch's end; the next one is where a gap lands to run the
        // default.
        ins.block(BlockType::Empty);
        cx.push(Lbl::Other);
        ins.block(BlockType::Empty);
        cx.push(Lbl::Other);
        for _ in 0..n {
            ins.block(BlockType::Empty);
            cx.push(Lbl::Other);
        }
        self.push_scrutinee(cx, ins, scrutinee);
        // From the innermost case block, the `n` case blocks are at depths `0..n`, the default
        // block at `n` and the switch's end at `n + 1`.
        let mut targets = vec![n as u32; width];
        for (i, case) in cases.iter().enumerate() {
            for key in &case.keys {
                match key {
                    CaseKey::Variant(k) => targets[*k] = i as u32,
                    CaseKey::Lit(_) => {
                        self.internal(format!("a literal case in a tag switch in {}", cx.f.name));
                        return;
                    }
                }
            }
        }
        ins.br_table(targets, n as u32);
        for (i, case) in cases.iter().enumerate() {
            cx.pop();
            ins.end();
            let body = case.body;
            self.seq(cx, ins, body, dst);
            // The case may fall through (`yield`). The switch's end is `n - i` labels out: the
            // `n - 1 - i` case blocks left and the default block are above it.
            ins.br((n - i) as u32);
        }
        cx.pop();
        ins.end();
        match default {
            Some(d) => self.seq(cx, ins, *d, dst),
            // Exhaustive without a default, as the verifier requires: the entry is not taken.
            None => {
                ins.unreachable();
            }
        }
        cx.pop();
        ins.end();
    }

    /// A switch as a chain of comparing `if` / `else`, nested once per case.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn switch_chain(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        cases: &[emela_lir::Case],
        i: usize,
        default: &Option<SeqId>,
        dst: Option<VarId>,
        scrutinee: Scrutinee,
    ) {
        if i >= cases.len() {
            match default {
                Some(d) => {
                    self.seq(cx, ins, *d, dst);
                }
                None => {
                    ins.unreachable();
                }
            }
            return;
        }
        let case = &cases[i];
        self.push_case_match(cx, ins, case, scrutinee);
        ins.if_(BlockType::Empty);
        cx.push(Lbl::Other);
        let body = case.body;
        self.seq(cx, ins, body, dst);
        ins.else_();
        self.switch_chain(cx, ins, cases, i + 1, default, dst, scrutinee);
        cx.pop();
        ins.end();
    }

    /// Pushes whether the scrutinee matches one of the case's keys: the comparison of each key,
    /// combined with `i32_or`. A case has one key unless the LIR pass joined cases whose bodies
    /// are the same sequence.
    pub(super) fn push_case_match(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        case: &emela_lir::Case,
        scrutinee: Scrutinee,
    ) {
        let literal = matches!(scrutinee, Scrutinee::Lit(_));
        for (i, key) in case.keys.iter().enumerate() {
            self.push_scrutinee(cx, ins, scrutinee);
            match key {
                CaseKey::Variant(k) => {
                    ins.i32_const(*k as i32);
                }
                CaseKey::Lit(c) => {
                    self.const_val(ins, c);
                }
            }
            if literal {
                match scrutinee {
                    Scrutinee::Lit(v) if cx.f.vars[v].repr == Repr::I64 => ins.i64_eq(),
                    _ => ins.i32_eq(),
                };
            } else {
                ins.i32_eq();
            }
            if i > 0 {
                ins.i32_or();
            }
        }
        if case.keys.is_empty() {
            ins.i32_const(0);
        }
    }

    pub(super) fn push_scrutinee(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        s: Scrutinee,
    ) {
        match s {
            Scrutinee::Tag(v) => self.tag_value(cx, ins, v),
            Scrutinee::Variant(v) | Scrutinee::Lit(v) => {
                ins.local_get(cx.local(v));
            }
            Scrutinee::TagZero => {
                ins.i32_const(0);
            }
        }
    }

    /// The variant number of a heap enum value: the tag of its descriptor. The runtime's `tag` is
    /// these three instructions, so the emitter writes them where it reads a tag.
    pub(super) fn tag_value(&mut self, cx: &FnCtx, ins: &mut InstructionSink, v: VarId) {
        ins.local_get(cx.local(v));
        ins.i32_load(mem(8, 4));
        ins.i32_load(mem(DESC_TAG, 4));
    }

    /// Loads slot `index` of the object in local `base`, as `repr`.
    pub(super) fn load_field(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        base: VarId,
        index: usize,
        repr: Repr,
    ) {
        let off = HDR + index as u32 * 8;
        ins.local_get(cx.local(base));
        match repr {
            Repr::I32 | Repr::Ref => {
                ins.i32_load(mem(off, 4));
            }
            Repr::I64 => {
                ins.i64_load(mem(off, 8));
            }
            Repr::F64 => {
                ins.f64_load(mem(off, 8));
            }
            Repr::Void => {}
        }
    }

    pub(super) fn alloc_value(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        desc: u32,
        args: &[Operand],
    ) {
        let Some(d) = dst else {
            self.internal(format!("allocation without a destination in {}", cx.f.name));
            return;
        };
        self.alloc(cx, ins, d, desc, args.len());
        for (i, a) in args.iter().enumerate() {
            self.store_slot(cx, ins, d, i, a);
        }
    }

    /// The `Make` `dst = …` of a statement whose old block is reused (C4/PR-042): the destination
    /// becomes the old variable, every old field that holds a reference is released, and the new
    /// fields are written into the same object.
    ///
    /// `opt::reuse` proved that the old variable has exactly one use, the removed drop, so no
    /// argument reads the old block and no other variable reaches it. A reference field is
    /// released with the drop the old block's own release would have run, before the new field is
    /// stored: the slot owns one reference and the new argument owns another (the `Make` consumes
    /// it), so the release cannot free the argument's value, even when both denote the same
    /// object. The constructor and the type are the same, so the descriptor, the tag, the number
    /// of slots and the reference slots are the ones the object already has. A nullary
    /// constructor is static data, which both values already are.
    pub(super) fn reuse_make(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        old: VarId,
        value: &emela_lir::Value,
    ) {
        let emela_lir::Value::Make { ctor, args } = value else {
            self.internal(format!(
                "a reuse of a value that is not a `make` in {}",
                cx.f.name
            ));
            return;
        };
        let Some(d) = dst else {
            self.internal(format!("reuse without a destination in {}", cx.f.name));
            return;
        };
        if enum_repr(self.core(), ctor.enum_id) == Repr::I32 {
            // The representation is the tag itself: there is no block.
            ins.local_get(cx.local(old));
            ins.local_set(cx.local(d));
            return;
        }
        if self.layout(cx.f.vars[d].ty).is_some() {
            // Held by value: there is no allocation to reuse; the copy is the whole statement.
            for a in args {
                self.operand(cx, ins, a);
            }
            self.assign(cx, ins, Some(d), Repr::Ref);
            return;
        }
        ins.local_get(cx.local(old));
        ins.local_set(cx.local(d));
        // The old slots own one reference each: give each back with the drop the removed `drop`
        // would have run, in slot order, before the slot is overwritten.
        let drop = self.rt("drop");
        for slot in self.ref_slots(cx.f, old, ctor.enum_id, ctor.variant) {
            ins.local_get(cx.local(d));
            ins.i32_load(mem(HDR + slot as u32 * 8, 4));
            ins.call(drop);
        }
        for (i, a) in args.iter().enumerate() {
            self.store_slot(cx, ins, d, i, a);
        }
    }

    /// The slots of the object of `old` that hold a reference, in slot order, from the fields of
    /// the constructor's variant and the type arguments of the object's type.
    ///
    /// The descriptor was made from the same field representations ([`Value::Make`] marks a slot
    /// a reference when its argument is `Repr::Ref`, and the verifier takes an argument's
    /// representation from the field type), so these are the slots the old block's drop would
    /// release.
    pub(super) fn ref_slots(
        &mut self,
        f: &Function,
        old: VarId,
        enum_id: EnumId,
        variant: usize,
    ) -> Vec<usize> {
        let reprs = {
            let core = self.core();
            match core.ty(f.vars[old].ty) {
                Type::Enum(id, args) if *id == enum_id => {
                    core.enums[enum_id].variants.get(variant).map(|v| {
                        v.tys
                            .iter()
                            .map(|&t| repr_of_subst(core, t, args))
                            .collect::<Vec<_>>()
                    })
                }
                _ => None,
            }
        };
        let Some(reprs) = reprs else {
            self.internal(format!("a reuse of an unexpected value in {}", f.name));
            return Vec::new();
        };
        reprs
            .iter()
            .enumerate()
            .filter(|(_, r)| **r == Repr::Ref)
            .map(|(i, _)| i)
            .collect()
    }

    pub(super) fn prim(&mut self, ins: &mut InstructionSink, op: PrimOp, ty: PrimTy) {
        use PrimOp::*;
        match ty {
            PrimTy::I32 => match op {
                Add => {
                    ins.i32_add();
                }
                Sub => {
                    ins.i32_sub();
                }
                Mul => {
                    ins.i32_mul();
                }
                Div => {
                    let f = self.rt("idiv");
                    ins.call(f);
                }
                Rem => {
                    let f = self.rt("irem");
                    ins.call(f);
                }
                Neg => {
                    let f = self.rt("ineg");
                    ins.call(f);
                }
                Not => {
                    ins.i32_eqz();
                }
                Eq => {
                    ins.i32_eq();
                }
                Ne => {
                    ins.i32_ne();
                }
                Lt => {
                    ins.i32_lt_s();
                }
                Le => {
                    ins.i32_le_s();
                }
                Gt => {
                    ins.i32_gt_s();
                }
                Ge => {
                    ins.i32_ge_s();
                }
            },
            PrimTy::I64 => match op {
                Add => {
                    ins.i64_add();
                }
                Sub => {
                    ins.i64_sub();
                }
                Mul => {
                    ins.i64_mul();
                }
                Div => {
                    let f = self.rt("ldiv");
                    ins.call(f);
                }
                Rem => {
                    let f = self.rt("lrem");
                    ins.call(f);
                }
                Neg => {
                    let f = self.rt("lneg");
                    ins.call(f);
                }
                Not => {
                    ins.i64_eqz();
                }
                Eq => {
                    ins.i64_eq();
                }
                Ne => {
                    ins.i64_ne();
                }
                Lt => {
                    ins.i64_lt_s();
                }
                Le => {
                    ins.i64_le_s();
                }
                Gt => {
                    ins.i64_gt_s();
                }
                Ge => {
                    ins.i64_ge_s();
                }
            },
            PrimTy::F64 => match op {
                Add => {
                    ins.f64_add();
                }
                Sub => {
                    ins.f64_sub();
                }
                Mul => {
                    ins.f64_mul();
                }
                Div => {
                    ins.f64_div();
                }
                Rem => {
                    let f = self.rt("fmod");
                    ins.call(f);
                }
                Neg => {
                    ins.f64_neg();
                }
                Not => {
                    ins.f64_const(0.0.into());
                    ins.f64_eq();
                }
                Eq => {
                    ins.f64_eq();
                }
                Ne => {
                    ins.f64_ne();
                }
                Lt => {
                    ins.f64_lt();
                }
                Le => {
                    ins.f64_le();
                }
                Gt => {
                    ins.f64_gt();
                }
                Ge => {
                    ins.f64_ge();
                }
            },
            PrimTy::String => match op {
                Eq => {
                    let f = self.rt("str_eq");
                    ins.call(f);
                }
                Ne => {
                    let f = self.rt("str_eq");
                    ins.call(f);
                    ins.i32_eqz();
                }
                _ => {
                    // Compare by code point (utf-8 byte order is the same).
                    let f = self.rt("str_cmp");
                    ins.call(f);
                    ins.i32_const(0);
                    match op {
                        Lt => {
                            ins.i32_lt_s();
                        }
                        Le => {
                            ins.i32_le_s();
                        }
                        Gt => {
                            ins.i32_gt_s();
                        }
                        Ge => {
                            ins.i32_ge_s();
                        }
                        _ => {}
                    }
                }
            },
        }
    }

    pub(super) fn call(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        callee: &Callee,
        args: &[Operand],
    ) {
        match callee {
            Callee::Direct(g) => {
                let m = self.m;
                let f = &m.functions[*g];
                // The result of a flattened function is one wasm result per scalar leaf.
                let ret_leaves = self.leaves(f.ret).map(|l| l.to_vec());
                let ret = self.repr(f.ret);
                // The arguments must push exactly the parameters the signature declares: one
                // value per scalar leaf of a by-value parameter, one for any other.
                let expected: usize = f
                    .params
                    .iter()
                    .map(|&p| self.leaves(f.vars[p].ty).map_or(1, |l| l.len()))
                    .sum();
                let pushed: usize = args.iter().map(|a| Self::operand_width(cx, a)).sum();
                if pushed != expected {
                    let (callee, caller) = (f.name.clone(), cx.f.name.clone());
                    self.internal(format!(
                        "{callee} takes {expected} wasm arguments in {caller} but the call \
                         pushes {pushed}"
                    ));
                    return;
                }
                for a in args {
                    self.operand(cx, ins, a);
                }
                let index = self.fn_index(*g);
                ins.call(index);
                self.assign_result(cx, ins, dst, ret, ret_leaves);
            }
            Callee::Indirect(c) => {
                let fty = match self.core().ty(cx.f.vars[*c].ty) {
                    Type::Fn(fty) => fty.clone(),
                    _ => {
                        self.internal(format!(
                            "calling a value that is not a function in {}",
                            cx.f.name
                        ));
                        return;
                    }
                };
                self.indirect(cx, ins, dst, *c, args, fty.ret);
            }
            Callee::Op {
                handler,
                effect,
                op,
            } => {
                let def = self.core().effects[*effect].ops[*op].clone();
                let ret = self.repr(def.ret);
                self.op_call(cx, ins, dst, *handler, *op, args, ret, &def);
            }
        }
    }

    /// A `call_indirect` of a closure. The closure record is one reference parameter (the
    /// environment); each other parameter and the result follow the layouts of the function type,
    /// so an indirect call has the same signature as the function it enters.
    pub(super) fn indirect(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        closure: VarId,
        args: &[Operand],
        ret_ty: TypeId,
    ) {
        let mut params = vec![val(Repr::Ref)];
        params.extend(self.operand_val_types(cx, args));
        let (results, ret_leaves) = self.result_val_types(ret_ty);
        let ty = self.ty(params, results);
        ins.local_get(cx.local(closure));
        for a in args {
            self.operand(cx, ins, a);
        }
        ins.local_get(cx.local(closure));
        ins.i32_load(mem(HDR, 4));
        ins.call_indirect(0, ty);
        let ret = self.repr(ret_ty);
        self.assign_result(cx, ins, dst, ret, ret_leaves);
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn op_call(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        handler: VarId,
        op: usize,
        args: &[Operand],
        ret: Repr,
        def: &emela_core::OpDef,
    ) {
        let suspend = def.suspend;
        // A `suspend fn` implementation takes the resume function last and returns nothing; the
        // value to continue with comes from `perform_end` (spec 8.4, 14.5). Its parameters follow
        // the layouts of the operation's function type (the implementation is a lifted closure of
        // that type); its result stays boxed, because the resume channel carries one word.
        let mut params = vec![val(Repr::Ref)];
        params.extend(self.operand_val_types(cx, args));
        if suspend {
            params.push(val(Repr::Ref));
        }
        let (results, ret_leaves) = if suspend {
            (Vec::new(), None)
        } else {
            self.result_val_types(def.ret)
        };
        let ty = self.ty(params, results);
        let op_off = HDR + 8 * (op as u32 + 2);
        let rs = if suspend {
            let rs = cx.scratch(Repr::Ref, 0);
            let prefix =
                self.string_literal(format!("operation `{}` of handler `", def.name).as_bytes());
            let resume = self.resume_table(ret);
            ins.i32_const(prefix as i32);
            ins.local_get(cx.local(handler));
            ins.i32_const(resume as i32);
            ins.i32_const(repr_code(ret));
            let f = self.rt("perform_new");
            ins.call(f);
            ins.local_set(rs);
            Some(rs)
        } else {
            None
        };
        ins.local_get(cx.local(handler));
        ins.i32_load(mem(op_off, 4));
        for a in args {
            self.operand(cx, ins, a);
        }
        if let Some(rs) = rs {
            ins.local_get(rs);
        }
        ins.local_get(cx.local(handler));
        ins.i32_load(mem(op_off, 4));
        ins.i32_load(mem(HDR, 4));
        ins.call_indirect(0, ty);
        if let Some(rs) = rs {
            let end = self.rt("perform_end");
            ins.local_get(rs);
            ins.call(end);
            self.perform_result(ins, ret);
            self.assign(cx, ins, dst, ret);
        } else {
            self.assign_result(cx, ins, dst, ret, ret_leaves);
        }
    }

    /// the run-time scope of a group, `fork`, `join`, the Deferred operations and the
    /// operations of `Std.Cause` (spec chapter 11), with the runtime of `runtime::fiber`.
    pub(super) fn fiber(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        op: FiberOp,
        args: &[Operand],
    ) {
        match op {
            // The Semaphore and Queue operations (`runtime::fiber::line`): the operands in order, and
            // the result, if any, in the destination.
            FiberOp::Semaphore
            | FiberOp::Queue
            | FiberOp::Acquire
            | FiberOp::TryAcquire
            | FiberOp::Release
            | FiberOp::Available
            | FiberOp::Take
            | FiberOp::QueuePoll
            | FiberOp::Size
            | FiberOp::Close => {
                let (name, ret) = match op {
                    FiberOp::Semaphore => ("fb_semaphore", Some(Repr::Ref)),
                    FiberOp::Queue => ("fb_queue", Some(Repr::Ref)),
                    FiberOp::Acquire => ("fb_acquire", None),
                    FiberOp::TryAcquire => ("fb_try_acquire", Some(Repr::I32)),
                    FiberOp::Release => ("fb_release", None),
                    FiberOp::Available => ("fb_available", Some(Repr::I32)),
                    FiberOp::Take => ("fb_take", Some(Repr::Ref)),
                    FiberOp::QueuePoll => ("fb_queue_poll", Some(Repr::Ref)),
                    FiberOp::Size => ("fb_queue_size", Some(Repr::I32)),
                    _ => ("fb_queue_close", None),
                };
                for a in args {
                    self.operand(cx, ins, a);
                }
                let f = self.rt(name);
                ins.call(f);
                if let Some(repr) = ret {
                    self.assign(cx, ins, dst, repr);
                }
            }
            FiberOp::Offer | FiberOp::TryOffer => {
                let repr = self.operand_repr_of(cx, &args[1]);
                self.operand(cx, ins, &args[0]);
                // A value without representation is passed as the zero word, as for complete.
                if repr == Repr::Void {
                    ins.i64_const(0);
                } else {
                    self.operand(cx, ins, &args[1]);
                    if repr == Repr::F64 {
                        ins.i64_reinterpret_f64();
                    } else if repr != Repr::I64 {
                        ins.i64_extend_i32_u();
                    }
                }
                ins.i32_const(repr_code(repr));
                let f = self.rt(if op == FiberOp::Offer {
                    "fb_offer"
                } else {
                    "fb_try_offer"
                });
                ins.call(f);
                self.assign(cx, ins, dst, Repr::I32);
            }
            FiberOp::Scope { background, .. } => {
                let ret = self.operand_fn_ret(cx, &args[0]);
                let repr = self.repr(ret);
                let table = self.thunk1(repr);
                for a in args {
                    self.operand(cx, ins, a);
                }
                ins.i32_const(table as i32);
                ins.i32_const(repr_code(repr));
                ins.i32_const(i32::from(background));
                let f = self.rt("fb_scope");
                ins.call(f);
                self.assign_i64(cx, ins, dst, repr);
            }
            FiberOp::Fork => {
                let ret = self.operand_fn_ret(cx, &args[1]);
                let repr = self.repr(ret);
                let table = self.thunk0(repr);
                for a in args {
                    self.operand(cx, ins, a);
                }
                ins.i32_const(table as i32);
                ins.i32_const(repr_code(repr));
                let f = self.rt("fb_fork");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            FiberOp::Deferred => {
                let f = self.rt("fb_deferred");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            FiberOp::Join => {
                let repr = self.dst_repr(cx, dst);
                self.operand(cx, ins, &args[0]);
                ins.i32_const(repr_code(repr));
                let f = self.rt("fb_join");
                ins.call(f);
                self.assign_i64(cx, ins, dst, repr);
            }
            FiberOp::Complete => {
                let repr = self.operand_repr_of(cx, &args[1]);
                self.operand(cx, ins, &args[0]);
                // A value without representation is passed as the zero word (the C runtime's
                // `em_deferred_complete`).
                if repr == Repr::Void {
                    ins.i64_const(0);
                } else {
                    self.operand(cx, ins, &args[1]);
                    if repr == Repr::F64 {
                        ins.i64_reinterpret_f64();
                    } else if repr != Repr::I64 {
                        ins.i64_extend_i32_u();
                    }
                }
                ins.i32_const(repr_code(repr));
                let f = self.rt("fb_complete");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::I32);
            }
            FiberOp::DeferredAwait => {
                let repr = self.dst_repr(cx, dst);
                self.operand(cx, ins, &args[0]);
                ins.i32_const(repr_code(repr));
                let f = self.rt("fb_deferred_await");
                ins.call(f);
                self.assign_i64(cx, ins, dst, repr);
            }
            FiberOp::Poll => {
                let (some, none, repr) = match dst {
                    Some(d) => self
                        .option_parts(cx.f.vars[d].ty)
                        .unwrap_or_else(|| panic!("Deferred.poll without an Option type")),
                    None => return,
                };
                self.operand(cx, ins, &args[0]);
                ins.i32_const(repr_code(repr));
                ins.i32_const(some as i32);
                ins.i32_const(none as i32);
                let f = self.rt("fb_poll");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            FiberOp::Attempt { suspends, .. } => {
                let Some(Operand::Var(f)) = args.first() else {
                    self.internal(format!("attempt without a function in {}", cx.f.name));
                    return;
                };
                let ret = match self.core().ty(cx.f.vars[*f].ty) {
                    Type::Fn(fty) => self.repr(fty.ret),
                    _ => {
                        self.internal(format!(
                            "attempt of a value that is not a function in {}",
                            cx.f.name
                        ));
                        return;
                    }
                };
                let word = cx.scratch(Repr::I64, 0);
                let exit = cx.scratch(Repr::Ref, 0);
                let ty = self.call_ty(&[Repr::Ref], ret);
                ins.local_get(cx.local(*f));
                ins.local_get(cx.local(*f));
                ins.i32_load(mem(HDR, 4));
                ins.call_indirect(0, ty);
                match ret {
                    Repr::I32 | Repr::Ref => {
                        ins.i64_extend_i32_u();
                    }
                    Repr::I64 => {}
                    Repr::F64 => {
                        ins.i64_reinterpret_f64();
                    }
                    Repr::Void => {
                        ins.i64_const(0);
                    }
                }
                ins.local_set(word);
                let success = runtime::fiber::attempt::success_desc(self, ret);
                if suspends {
                    let frame = runtime::fiber::attempt::frame_desc(self);
                    let table = self.sus.as_ref().map_or(0, |p| p.attempt_table);
                    ins.global_get(G_SUSPENDING);
                    ins.if_(BlockType::Result(ValType::I32));
                    ins.i32_const(frame as i32);
                    ins.i32_const(table as i32);
                    ins.i32_const(success as i32);
                    ins.i32_const(0);
                    let new = self.rt("frame_new");
                    ins.call(new);
                    let push = self.rt("frame_push");
                    ins.call(push);
                    ins.i32_const(0);
                    ins.else_();
                }
                let caught = self.rt("attempt_caught");
                ins.call(caught);
                ins.local_tee(exit);
                ins.if_(BlockType::Result(ValType::I32));
                ins.local_get(exit);
                ins.else_();
                ins.global_get(G_DEFECT);
                ins.if_(BlockType::Result(ValType::I32));
                ins.i32_const(0);
                ins.else_();
                ins.i32_const(success as i32);
                ins.local_get(word);
                let make = self.rt("exit_success");
                ins.call(make);
                ins.end();
                ins.end();
                if suspends {
                    ins.end();
                }
                self.assign(cx, ins, dst, Repr::Ref);
            }
            FiberOp::CauseKind { kind } => {
                if enum_repr(self.core(), kind) != Repr::I32 {
                    self.internal("`CauseKind` is not an enum of plain variants");
                    return;
                }
                self.operand(cx, ins, &args[0]);
                ins.i32_load(mem(4, 4));
                self.assign(cx, ins, dst, Repr::I32);
            }
            FiberOp::CauseToString => {
                self.operand(cx, ins, &args[0]);
                let f = self.rt("cause_to_string");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            FiberOp::CauseRaise { .. } => {
                self.operand(cx, ins, &args[0]);
                let f = self.rt("cause_raise");
                ins.call(f);
            }
        }
    }

    pub(super) fn operand_fn_ret(&self, cx: &FnCtx<'_>, op: &Operand) -> TypeId {
        match op {
            Operand::Var(v) => match self.core().ty(cx.f.vars[*v].ty) {
                Type::Fn(f) => f.ret,
                _ => TypeId::UNIT,
            },
            Operand::Const(_) => TypeId::UNIT,
        }
    }

    pub(super) fn operand_repr_of(&self, cx: &FnCtx<'_>, op: &Operand) -> Repr {
        match op {
            Operand::Var(v) => self.repr(cx.f.vars[*v].ty),
            Operand::Const(c) => match c {
                emela_lir::Const::Int(_) | emela_lir::Const::Bool(_) => Repr::I32,
                emela_lir::Const::Int64(_) => Repr::I64,
                emela_lir::Const::Float(_) => Repr::F64,
                emela_lir::Const::String(_) => Repr::Ref,
                emela_lir::Const::Unit => Repr::Void,
            },
        }
    }

    /// The representation of a `join` or `Deferred.await` result. Without a destination the value
    /// is unused; it is read as a reference and released.
    pub(super) fn dst_repr(&self, cx: &FnCtx<'_>, dst: Option<VarId>) -> Repr {
        match dst {
            Some(d) => self.repr(cx.f.vars[d].ty),
            None => Repr::Ref,
        }
    }

    /// The table index of the closure thunk for `repr` (`fn() -> A`).
    pub(super) fn thunk0(&self, repr: Repr) -> u32 {
        let plan = self
            .fiber_plan
            .as_ref()
            .expect("the fiber runtime for a forked fiber");
        plan.call_table + repr_code(repr) as u32
    }

    /// The table index of the closure thunk for `repr` (`fn(Scope) -> A`).
    pub(super) fn thunk1(&self, repr: Repr) -> u32 {
        let plan = self
            .fiber_plan
            .as_ref()
            .expect("the fiber runtime for a scope");
        plan.call1_table + repr_code(repr) as u32
    }

    /// Stores the `i64` word a fiber operation returned, as `repr`.
    pub(super) fn assign_i64(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        repr: Repr,
    ) {
        match repr {
            Repr::I32 | Repr::Ref => {
                ins.i32_wrap_i64();
            }
            Repr::I64 => {}
            Repr::F64 => {
                ins.f64_reinterpret_i64();
            }
            Repr::Void => {
                ins.drop();
                return;
            }
        }
        self.assign(cx, ins, dst, repr);
    }

    /// The resume function table index for an operation returning `ret`.
    pub(super) fn resume_table(&mut self, ret: Repr) -> u32 {
        match &self.sus {
            Some(plan) => plan.resume_table(ret),
            None => {
                self.internal("a `suspend fn` operation without the suspend runtime");
                0
            }
        }
    }

    /// Leaves the value `perform_end` produced (or a dummy when it suspended or a pending
    /// failure or defect is set).
    pub(super) fn perform_result(&mut self, ins: &mut InstructionSink, ret: Repr) {
        if ret == Repr::Void {
            return;
        }
        let Some(plan) = &self.sus else { return };
        let out_mode = plan.out_mode;
        ins.i32_const(out_mode as i32);
        ins.i32_load8_u(mem(0, 1));
        ins.i32_eqz();
        ins.if_(BlockType::Result(val(ret)));
        ins.i32_const(plan.out_word as i32);
        match ret {
            Repr::I32 | Repr::Ref => {
                ins.i32_load(mem(0, 4));
            }
            Repr::I64 => {
                ins.i64_load(mem(0, 8));
            }
            Repr::F64 => {
                ins.f64_load(mem(0, 8));
            }
            Repr::Void => unreachable!(),
        }
        ins.else_();
        match ret {
            Repr::I32 | Repr::Ref => {
                ins.i32_const(0);
            }
            Repr::I64 => {
                ins.i64_const(0);
            }
            Repr::F64 => {
                ins.f64_const(0.0.into());
            }
            Repr::Void => unreachable!(),
        }
        ins.end();
    }
}
