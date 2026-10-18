//! The suspension state machines of the Heap IR (spec chapters 11 and 14).
//!
//! A suspending function with suspension points is emitted as a state machine: its body is
//! entered normally (no frame, `state == -1`) or by the driver loop with a frame (the resume
//! point). While a resume is under way every statement checks the `state` local and either skips
//! (the resume point is later), enters (it contains the resume point), or records the resume
//! input. When a value comes back suspended (`G_SUSPENDING`) the function saves the live variables
//! in a frame of its own, pushes it onto the captured continuation and returns to its caller; the
//! driver later restores the frame and continues right after the value.
//!
//! The frame layout (an ordinary heap object so `drop` frees an abandoned continuation):
//!
//! ```text
//! header (HDR) | resume table index | state | next frame | saved 0 | saved 1 | ...
//! ```
//!
//! The resume input is the `i64` global `G_RESUME` (a value; a pending failure or defect is in
//! `G_FAILURE`/`G_DEFECT`), exactly as `runtime::sched` documents.

use super::*;

impl Machine {
    /// The state machine of a suspending function: the state range of every statement and sequence
    /// that contains a suspension point, the resume points, and the variables they restore.
    pub(crate) fn build(f: &Function, table: u32) -> Machine {
        let mut m = Machine {
            stmt_range: FxHashMap::default(),
            seq_range: FxHashMap::default(),
            edges: FxHashMap::default(),
            resumes: Vec::new(),
            table,
        };
        if let Some(body) = f.body {
            m.range_seq(f, body);
        }
        let mut edges: Vec<(u32, Vec<VarId>)> = m
            .edges
            .values()
            .map(|e| (e.state, e.saved.clone()))
            .collect();
        edges.sort_by_key(|(s, _)| *s);
        m.resumes = edges;
        m
    }

    fn range_seq(&mut self, f: &Function, seq: SeqId) -> Option<(u32, u32)> {
        let mut range = None;
        for s in f.seqs[seq].stmts.clone() {
            range = combine(range, self.range_stmt(f, s));
        }
        if let Some(r) = range {
            self.seq_range.insert(seq, r);
        }
        range
    }

    fn range_stmt(&mut self, f: &Function, id: StmtId) -> Option<(u32, u32)> {
        let stmt = f.stmts[id].clone();
        let mut range = None;
        let children: Vec<SeqId> = match &stmt {
            Stmt::Let {
                value,
                on_defect,
                on_suspend,
                ..
            } => {
                if let Some(e) = on_suspend {
                    self.edges.insert(id, e.clone());
                    range = Some((e.state, e.state));
                }
                let mut out = Vec::new();
                if let Value::Call {
                    on_fail: Some(OnFail::Handle { body, .. }),
                    ..
                }
                | Value::Fiber {
                    on_fail: Some(OnFail::Handle { body, .. }),
                    ..
                } = value
                {
                    out.push(*body);
                }
                out.extend(*on_defect);
                out
            }
            Stmt::If { then, else_, .. } => vec![*then, *else_],
            Stmt::Switch { cases, default, .. } => cases
                .iter()
                .map(|c| c.body)
                .chain(default.iter().copied())
                .collect(),
            Stmt::Loop { body, .. } | Stmt::Block { body, .. } => vec![*body],
            Stmt::Protect { body, cleanup, .. } => vec![*body, *cleanup],
            Stmt::Dup(_) | Stmt::Drop(_) => Vec::new(),
        };
        for c in children {
            range = combine(range, self.range_seq(f, c));
        }
        if let Some(r) = range {
            self.stmt_range.insert(id, r);
        }
        range
    }
}

fn combine(a: Option<(u32, u32)>, b: Option<(u32, u32)>) -> Option<(u32, u32)> {
    match (a, b) {
        (None, x) | (x, None) => x,
        (Some((alo, ahi)), Some((blo, bhi))) => Some((alo.min(blo), ahi.max(bhi))),
    }
}

impl<'m> Builder<'m> {
    /// The state-machine body `f_sm`: entered without a frame by the function itself and with one
    /// by the driver loop.
    pub(super) fn emit_machine_body(
        &mut self,
        id: FnId,
        f: &Function,
        info: MachineInfo,
    ) -> WasmFunction {
        let machine = Machine::build(f, info.table);
        let mut cx = FnCtx::new_machine(f, self, machine);
        let mut func = WasmFunction::new(cx.decls.clone());
        {
            let mut ins = func.instructions();
            self.machine_entry(&mut cx, &mut ins, info);
            let body = f.body.expect("a state machine has a body");
            self.seq(&mut cx, &mut ins, body, None);
            let ret = cx.ret;
            dummy_return(&mut ins, ret);
            ins.end();
        }
        let _ = id;
        func
    }

    /// The entry: with a frame, restore the saved variables of its resume point, free it and set
    /// the `state` local to skip the body to that point; without one, run from the top.
    fn machine_entry(&mut self, cx: &mut FnCtx, ins: &mut InstructionSink, _info: MachineInfo) {
        let resumes = cx.machine.as_ref().unwrap().resumes.clone();
        let frame = cx.frame;
        ins.local_get(frame);
        ins.i32_eqz();
        ins.if_(BlockType::Empty);
        ins.i32_const(-1);
        ins.local_set(cx.state);
        ins.else_();
        ins.local_get(frame);
        ins.i32_load(mem(FRAME_STATE, 4));
        ins.local_set(cx.state);
        for (state, saved) in &resumes {
            ins.local_get(cx.state);
            ins.i32_const(*state as i32);
            ins.i32_eq();
            ins.if_(BlockType::Empty);
            for (i, &v) in saved.iter().enumerate() {
                ins.local_get(frame);
                load_slot(ins, cx.f.vars[v].repr, FRAME_SAVED + 8 * i as u32);
                ins.local_set(cx.local(v));
            }
            ins.end();
        }
        ins.local_get(frame);
        let free = self.rt("frame_free");
        ins.call(free);
        ins.end();
    }

    /// Pushes `state >= lo && state <= hi` (a bool) onto the stack.
    pub(super) fn machine_in_range(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        lo: u32,
        hi: u32,
    ) {
        ins.local_get(cx.state);
        ins.i32_const(lo as i32);
        ins.i32_ge_s();
        ins.local_get(cx.state);
        ins.i32_const(hi as i32);
        ins.i32_le_s();
        ins.i32_and();
    }

    /// A statement of a state machine: it runs normally only while `state == -1`, or while a
    /// resume is looking for a point inside it.
    pub(super) fn machine_stmt(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        id: StmtId,
        stmt: Stmt,
    ) {
        match stmt {
            Stmt::Let {
                dst,
                value,
                on_defect,
                on_suspend,
            } => self.machine_let(cx, ins, id, dst, value, on_defect, on_suspend),
            Stmt::If {
                dst,
                cond,
                then,
                else_,
            } => {
                let then_range = cx.machine.as_ref().unwrap().seq_range.get(&then).copied();
                let else_range = cx.machine.as_ref().unwrap().seq_range.get(&else_).copied();
                if then_range.is_none() && else_range.is_none() {
                    self.guard_default(cx, ins, |b, cx, ins| {
                        b.operand(cx, ins, &cond);
                        ins.if_(BlockType::Empty);
                        cx.push(Lbl::Other);
                        b.seq(cx, ins, then, dst);
                        ins.else_();
                        b.seq(cx, ins, else_, dst);
                        cx.pop();
                        ins.end();
                    });
                    return;
                }
                let (tlo, thi) = then_range.unwrap_or((1, 0));
                let (elo, ehi) = else_range.unwrap_or((1, 0));
                ins.block(BlockType::Empty);
                cx.push(Lbl::Other);
                let skip = cx.labels.len() - 1;
                ins.local_get(cx.state);
                ins.i32_const(-1);
                ins.i32_eq();
                ins.if_(BlockType::Result(ValType::I32));
                cx.push(Lbl::Other);
                self.operand(cx, ins, &cond);
                ins.else_();
                self.machine_in_range(cx, ins, tlo, thi);
                ins.if_(BlockType::Result(ValType::I32));
                cx.push(Lbl::Other);
                ins.i32_const(1);
                ins.else_();
                self.machine_in_range(cx, ins, elo, ehi);
                ins.if_(BlockType::Result(ValType::I32));
                cx.push(Lbl::Other);
                ins.i32_const(0);
                ins.else_();
                let depth = (cx.labels.len() - 1 - skip) as u32;
                ins.br(depth);
                cx.pop();
                ins.end();
                cx.pop();
                ins.end();
                cx.pop();
                ins.end();
                ins.local_set(cx.take);
                ins.local_get(cx.take);
                ins.if_(BlockType::Empty);
                cx.push(Lbl::Other);
                self.seq(cx, ins, then, dst);
                ins.else_();
                self.seq(cx, ins, else_, dst);
                cx.pop();
                ins.end();
                cx.pop();
                ins.end();
            }
            Stmt::Loop {
                dst,
                vars,
                inits,
                body,
            } => {
                let range = cx.machine.as_ref().unwrap().seq_range.get(&body).copied();
                if range.is_none() {
                    self.guard_default(cx, ins, |b, cx, ins| {
                        for (&v, init) in vars.iter().zip(&inits) {
                            b.operand(cx, ins, init);
                            ins.local_set(cx.local(v));
                        }
                        ins.loop_(BlockType::Empty);
                        cx.push(Lbl::Loop(vars.clone()));
                        b.seq(cx, ins, body, dst);
                        cx.pop();
                        ins.end();
                    });
                    return;
                }
                let (lo, hi) = range.unwrap();
                // The inits run only on a normal entry (a resume restores the loop variables).
                ins.local_get(cx.state);
                ins.i32_const(-1);
                ins.i32_eq();
                ins.if_(BlockType::Empty);
                cx.push(Lbl::Other);
                for (&v, init) in vars.iter().zip(&inits) {
                    self.operand(cx, ins, init);
                    ins.local_set(cx.local(v));
                }
                cx.pop();
                ins.end();
                ins.local_get(cx.state);
                ins.i32_const(-1);
                ins.i32_eq();
                self.machine_in_range(cx, ins, lo, hi);
                ins.i32_or();
                ins.if_(BlockType::Empty);
                cx.push(Lbl::Other);
                ins.loop_(BlockType::Empty);
                cx.push(Lbl::Loop(vars.clone()));
                self.seq(cx, ins, body, dst);
                cx.pop();
                ins.end();
                cx.pop();
                ins.end();
            }
            Stmt::Block { dst, label, body } => {
                let range = cx.machine.as_ref().unwrap().seq_range.get(&body).copied();
                if range.is_none() {
                    self.guard_default(cx, ins, |b, cx, ins| {
                        ins.block(BlockType::Empty);
                        cx.push(Lbl::Block(label, dst));
                        b.seq(cx, ins, body, dst);
                        cx.pop();
                        ins.end();
                    });
                    return;
                }
                let (lo, hi) = range.unwrap();
                ins.block(BlockType::Empty);
                cx.push(Lbl::Other);
                let skip = cx.labels.len() - 1;
                ins.local_get(cx.state);
                ins.i32_const(-1);
                ins.i32_eq();
                self.machine_in_range(cx, ins, lo, hi);
                ins.i32_or();
                ins.if_(BlockType::Empty);
                cx.push(Lbl::Other);
                ins.block(BlockType::Empty);
                cx.push(Lbl::Block(label, dst));
                self.seq(cx, ins, body, dst);
                cx.pop();
                ins.end();
                ins.else_();
                let depth = (cx.labels.len() - 1 - skip) as u32;
                ins.br(depth);
                cx.pop();
                ins.end();
                cx.pop();
                ins.end();
            }
            Stmt::Switch {
                dst,
                on,
                cases,
                default,
            } => self.machine_switch(cx, ins, dst, on, cases, default),
            Stmt::Protect { .. } => self.internal(format!("protect left in {}", cx.f.name)),
            Stmt::Dup(v) => self.guard_default(cx, ins, |b, cx, ins| {
                ins.local_get(cx.local(v));
                let dup = b.rt("dup");
                ins.call(dup);
            }),
            Stmt::Drop(v) => self.guard_default(cx, ins, |b, cx, ins| {
                ins.local_get(cx.local(v));
                let drop = b.rt("drop");
                ins.call(drop);
            }),
        }
    }

    /// Emits a statement that has no suspension point of its own only when `state == -1`.
    fn guard_default(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        body: impl FnOnce(&mut Self, &mut FnCtx, &mut InstructionSink),
    ) {
        ins.local_get(cx.state);
        ins.i32_const(-1);
        ins.i32_eq();
        ins.if_(BlockType::Empty);
        cx.push(Lbl::Other);
        body(self, cx, ins);
        cx.pop();
        ins.end();
    }

    #[allow(clippy::too_many_arguments)]
    fn machine_let(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        id: StmtId,
        dst: Option<VarId>,
        value: Value,
        on_defect: Option<SeqId>,
        on_suspend: Option<emela_lir::Suspend>,
    ) {
        let range = cx.machine.as_ref().unwrap().stmt_range.get(&id).copied();
        if range.is_none() {
            self.guard_default(cx, ins, |b, cx, ins| {
                b.value(cx, ins, dst, &value);
                b.let_landings(cx, ins, &value, on_defect, None);
            });
            return;
        }
        let (lo, hi) = range.unwrap();
        let own = on_suspend.as_ref().map(|e| e.state);
        let landing =
            on_defect.and_then(|s| cx.machine.as_ref().unwrap().seq_range.get(&s).copied());
        ins.block(BlockType::Empty);
        cx.push(Lbl::Other);
        let skip = cx.labels.len() - 1;
        ins.local_get(cx.state);
        ins.i32_const(-1);
        ins.i32_eq();
        ins.if_(BlockType::Empty);
        cx.push(Lbl::Other);
        self.value(cx, ins, dst, &value);
        if let Some(edge) = &on_suspend {
            self.suspend_edge(cx, ins, edge);
        }
        ins.else_();
        self.machine_in_range(cx, ins, lo, hi);
        ins.if_(BlockType::Empty);
        cx.push(Lbl::Other);
        if let Some(s) = own {
            ins.local_get(cx.state);
            ins.i32_const(s as i32);
            ins.i32_eq();
            ins.if_(BlockType::Empty);
            cx.push(Lbl::Other);
            self.bind_resume(cx, ins, dst);
            cx.pop();
            ins.end();
        }
        ins.else_();
        let depth = (cx.labels.len() - 1 - skip) as u32;
        ins.br(depth);
        cx.pop();
        ins.end();
        cx.pop();
        ins.end();
        self.let_landings(cx, ins, &value, on_defect, landing);
        cx.pop();
        ins.end();
    }

    /// A switch of a state machine: the resume selects the case whose range holds the state (and
    /// skips the whole switch if none does), the normal path evaluates the scrutinee as usual.
    #[allow(clippy::too_many_arguments)]
    fn machine_switch(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        on: SwitchOn,
        cases: Vec<emela_lir::Case>,
        default: Option<SeqId>,
    ) {
        let any = cases
            .iter()
            .any(|c| cx.machine.as_ref().unwrap().seq_range.contains_key(&c.body))
            || default.is_some_and(|d| cx.machine.as_ref().unwrap().seq_range.contains_key(&d));
        let scrutinee = match on {
            SwitchOn::Tag(v) => {
                if cx.f.vars[v].repr == Repr::Ref {
                    Scrutinee::Tag(v)
                } else {
                    Scrutinee::Lit(v)
                }
            }
            SwitchOn::Lit(v) => Scrutinee::Lit(v),
        };
        if !any {
            self.guard_default(cx, ins, |b, cx, ins| {
                b.switch_chain(cx, ins, &cases, 0, &default, dst, scrutinee);
            });
            return;
        }
        let ranges: Vec<(u32, u32)> = cases
            .iter()
            .map(|c| {
                cx.machine
                    .as_ref()
                    .unwrap()
                    .seq_range
                    .get(&c.body)
                    .copied()
                    .unwrap_or((1, 0))
            })
            .collect();
        let drange = default.and_then(|d| cx.machine.as_ref().unwrap().seq_range.get(&d).copied());
        let n = cases.len();
        ins.block(BlockType::Empty);
        cx.push(Lbl::Other);
        let skip = cx.labels.len() - 1;
        ins.local_get(cx.state);
        ins.i32_const(-1);
        ins.i32_eq();
        ins.if_(BlockType::Empty);
        cx.push(Lbl::Other);
        // Normal: the default index, then the matching case.
        ins.i32_const(n as i32);
        ins.local_set(cx.take);
        for (i, case) in cases.iter().enumerate() {
            self.push_scrutinee(cx, ins, scrutinee);
            match &case.key {
                CaseKey::Variant(k) => {
                    ins.i32_const(*k as i32);
                }
                CaseKey::Lit(c) => self.const_val(ins, c),
            }
            match scrutinee {
                Scrutinee::Tag(_) => ins.i32_eq(),
                Scrutinee::Lit(v) => match cx.f.vars[v].repr {
                    Repr::I64 => ins.i64_eq(),
                    _ => ins.i32_eq(),
                },
            };
            ins.if_(BlockType::Empty);
            cx.push(Lbl::Other);
            ins.i32_const(i as i32);
            ins.local_set(cx.take);
            cx.pop();
            ins.end();
        }
        ins.else_();
        // Resuming: the case whose range holds the state.
        ins.i32_const(-1);
        ins.local_set(cx.take);
        for (i, (lo, hi)) in ranges.iter().enumerate() {
            self.machine_in_range(cx, ins, *lo, *hi);
            ins.if_(BlockType::Empty);
            cx.push(Lbl::Other);
            ins.i32_const(i as i32);
            ins.local_set(cx.take);
            cx.pop();
            ins.end();
        }
        if let Some((lo, hi)) = drange {
            self.machine_in_range(cx, ins, lo, hi);
            ins.if_(BlockType::Empty);
            cx.push(Lbl::Other);
            ins.i32_const(n as i32);
            ins.local_set(cx.take);
            cx.pop();
            ins.end();
        }
        ins.local_get(cx.take);
        ins.i32_const(0);
        ins.i32_lt_s();
        ins.if_(BlockType::Empty);
        cx.push(Lbl::Other);
        let depth = (cx.labels.len() - 1 - skip) as u32;
        ins.br(depth);
        cx.pop();
        ins.end();
        cx.pop();
        ins.end();
        // Run the selected body (the cases, then the default).
        for (i, case) in cases.iter().enumerate() {
            ins.local_get(cx.take);
            ins.i32_const(i as i32);
            ins.i32_eq();
            ins.if_(BlockType::Empty);
            cx.push(Lbl::Other);
            self.seq(cx, ins, case.body, dst);
            ins.else_();
        }
        match default {
            Some(d) => self.seq(cx, ins, d, dst),
            None => {
                ins.unreachable();
            }
        }
        for _ in 0..n {
            cx.pop();
            ins.end();
        }
        cx.pop();
        ins.end();
    }

    /// Binds the resume input to `dst` and marks the resume as reached.
    fn bind_resume(&mut self, cx: &mut FnCtx, ins: &mut InstructionSink, dst: Option<VarId>) {
        if let Some(d) = dst {
            match cx.f.vars[d].repr {
                Repr::I32 | Repr::Ref => {
                    ins.global_get(G_RESUME);
                    ins.i32_wrap_i64();
                    ins.local_set(cx.local(d));
                }
                Repr::I64 => {
                    ins.global_get(G_RESUME);
                    ins.local_set(cx.local(d));
                }
                Repr::F64 => {
                    ins.global_get(G_RESUME);
                    ins.f64_reinterpret_i64();
                    ins.local_set(cx.local(d));
                }
                Repr::Void => {}
            }
        }
        ins.i32_const(-1);
        ins.local_set(cx.state);
    }

    /// The suspension edge of a value: when it came back suspended, save the live variables in a
    /// frame, push it onto the continuation and return.
    fn suspend_edge(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        edge: &emela_lir::Suspend,
    ) {
        let mut refs = vec![false, false, true];
        refs.extend(edge.saved.iter().map(|&v| cx.f.vars[v].repr == Repr::Ref));
        let name = check::display_name(&cx.f.name).to_owned();
        let desc = self.desc(DescKey {
            kind: 0,
            tag: 0,
            enum_id: -1,
            name,
            show: None,
            refs,
        });
        let table = cx.machine.as_ref().unwrap().table;
        let tmp = cx.scratch(Repr::Ref, 0);
        ins.global_get(G_SUSPENDING);
        ins.if_(BlockType::Empty);
        cx.push(Lbl::Other);
        ins.i32_const(desc as i32);
        ins.i32_const(table as i32);
        ins.i32_const(edge.state as i32);
        ins.i32_const(edge.saved.len() as i32);
        let new = self.rt("frame_new");
        ins.call(new);
        ins.local_set(tmp);
        for (i, &v) in edge.saved.iter().enumerate() {
            ins.local_get(tmp);
            ins.local_get(cx.local(v));
            store_slot(ins, cx.f.vars[v].repr, FRAME_SAVED + 8 * i as u32);
        }
        ins.local_get(tmp);
        let push = self.rt("frame_push");
        ins.call(push);
        dummy_return(ins, cx.ret);
        cx.pop();
        ins.end();
    }

    /// A call of a `suspend fn` operation: the implementation takes a resume value last, and
    /// `perform_end` gives the value to continue with (or starts the suspension).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn suspend_op_call(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        handler: VarId,
        op: usize,
        op_name: &str,
        args: &[Operand],
        ret: Repr,
    ) {
        let op_off = HDR + 8 * (op as u32 + 2);
        let name = self.string_literal(op_name.as_bytes());
        let mut params = vec![Repr::Ref];
        params.extend(args.iter().map(|a| Self::operand_repr(cx.f, a)));
        params.push(Repr::Ref);
        let ty = self.call_ty(&params, Repr::Void);
        let rs = cx.scratch(Repr::Ref, 0);
        // If the handler was released, its operations are a defect (spec 8.6).
        ins.local_get(cx.local(handler));
        ins.i32_load(mem(HDR, 4));
        ins.if_(BlockType::Empty);
        ins.local_get(cx.local(handler));
        ins.i32_load(mem(HDR + 8, 4));
        let raise = self.rt("raise");
        ins.call(raise);
        dummy_return(ins, cx.ret);
        ins.else_();
        ins.local_get(cx.local(handler));
        ins.i32_const(name as i32);
        ins.i32_const(repr_code(ret) as i32);
        let new = self.rt("resume_new");
        ins.call(new);
        ins.local_set(rs);
        ins.local_get(cx.local(handler));
        ins.i32_load(mem(op_off, 4));
        for a in args {
            self.operand(cx, ins, a);
        }
        ins.local_get(rs);
        ins.local_get(cx.local(handler));
        ins.i32_load(mem(op_off, 4));
        ins.i32_load(mem(HDR, 4));
        ins.call_indirect(0, ty);
        ins.local_get(rs);
        let end = self.rt("perform_end");
        ins.call(end);
        match ret {
            Repr::Void => {
                ins.drop();
            }
            Repr::I32 | Repr::Ref => {
                ins.i32_wrap_i64();
            }
            Repr::I64 => {}
            Repr::F64 => {
                ins.f64_reinterpret_i64();
            }
        }
        self.assign(cx, ins, dst, ret);
        ins.end();
    }
}

/// `slot = load(base + off)` for a value of the representation (no store for `()`).
fn load_slot(ins: &mut InstructionSink, repr: Repr, off: u32) {
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

/// `store(load(base + off), value)` for a value of the representation (no store for `()`).
fn store_slot(ins: &mut InstructionSink, repr: Repr, off: u32) {
    match repr {
        Repr::I32 | Repr::Ref => {
            ins.i32_store(mem(off, 4));
        }
        Repr::I64 => {
            ins.i64_store(mem(off, 8));
        }
        Repr::F64 => {
            ins.f64_store(mem(off, 8));
        }
        Repr::Void => {}
    }
}
