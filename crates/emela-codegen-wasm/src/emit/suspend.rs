//! Suspending functions, their state machines and the frames of a captured continuation.
//!
//! A suspending function is compiled twice: as an ordinary wrapper (which the IR calls and the
//! closure table holds), and as the body of a state machine taking its frame pointer after its
//! parameters. Each suspension point (`on_suspend`) gets a resume point; when the value comes
//! back suspended, the body allocates a frame holding the resume point and the variables the
//! edge saves, pushes it onto the continuation being captured and returns. The runtime resumes
//! a frame by calling the function's trampoline, which enters the state machine with the frame;
//! the body restores the saved variables, frees the frame and continues at the resume point.
//!
//! The state machine is emitted as a flat dispatch loop: every statement sequence is a chunk of
//! instructions ending in a transition (a state assignment and a branch back to the loop head)
//! or in a return. WebAssembly has no `goto` into nested blocks, so all structured code of the
//! source becomes explicit states. The `br` depth of a transition could only be known once every
//! chunk exists, so a transition records its position and the assembler writes the branch after
//! the whole body is built.
//!
//! The frames and the scheduler live in `runtime::sched`; this module only reads the Heap IR.

use super::*;

use emela_lir::{
    CaseKey, LabelId, Operand, RuntimeOp, SeqId, Stmt, Suspend, SwitchOn, Value, VarId,
};
use wasm_encoder::Function as WasmFunction;

/// The extra definitions of one state machine.
#[derive(Clone, Copy)]
pub(crate) struct MachinePlan {
    /// The function whose body is the state machine.
    pub(crate) id: FnId,
    /// The state-machine body: its parameters then the frame pointer.
    pub(crate) body: u32,
    /// The trampoline's table index (stored in a frame).
    pub(crate) table: u32,
}

/// The suspend runtime's statics and the extra definitions of the state machines.
pub(crate) struct Plan {
    pub(crate) chain_head: u32,
    pub(crate) chain_tail: u32,
    pub(crate) in_mode: u32,
    pub(crate) in_word: u32,
    pub(crate) out_mode: u32,
    pub(crate) out_word: u32,
    /// The outermost operation of the current suspension cascade (see `runtime::async_bridge`).
    pub(crate) pending: u32,
    /// The innermost active operation, whose implementation the event resumes (see
    /// `runtime::async_bridge`).
    pub(crate) sleep_op: u32,
    pub(crate) queue: u32,
    pub(crate) qhead: u32,
    pub(crate) qlen: u32,
    pub(crate) sync_depth: u32,
    /// The descriptor of a `perform` record (the state of an operation call).
    pub(crate) perform_desc: u32,
    /// The descriptor of a resume closure (slot 1 is the `perform`).
    pub(crate) resume_desc: u32,
    /// The static `interrupted` value (compared by pointer, like the C runtime).
    pub(crate) interrupted: u32,
    /// The static pieces of the two resume diagnostics.
    pub(crate) twice_prefix: u32,
    pub(crate) after_prefix: u32,
    pub(crate) queue_full: u32,
    /// The resume functions' table indices, by `repr_index`.
    resume_table: [u32; 5],
    machines: Vec<MachinePlan>,
}

fn repr_index(r: Repr) -> usize {
    match r {
        Repr::Void => 0,
        Repr::I32 => 1,
        Repr::I64 => 2,
        Repr::F64 => 3,
        Repr::Ref => 4,
    }
}

impl Plan {
    pub(crate) fn resume_table(&self, r: Repr) -> u32 {
        self.resume_table[repr_index(r)]
    }

    pub(crate) fn machine(&self, id: FnId) -> Option<&MachinePlan> {
        // `machines` is filled in function-arena order.
        let key = id.into_raw();
        self.machines
            .binary_search_by_key(&key, |m| m.id.into_raw())
            .ok()
            .map(|i| &self.machines[i])
    }
}

/// Whether a function is emitted as a state machine: a suspending function with suspension
/// points.
pub(crate) fn is_machine(f: &Function) -> bool {
    f.suspends
        && f.body.is_some()
        && f.stmts.iter().any(|(_, s)| {
            matches!(
                s,
                Stmt::Let {
                    on_suspend: Some(_),
                    ..
                }
            )
        })
}

/// Whether the program needs the suspend runtime at all: any suspending function, or any value
/// that may suspend (a `suspend fn` operation, a suspending call, a fairness yield).
pub(crate) fn used(b: &Builder<'_>) -> bool {
    b.m.functions.iter().any(|(_, f)| {
        f.suspends
            || f.stmts
                .iter()
                .any(|(_, s)| matches!(s, Stmt::Let { value, .. } if emela_lir::may_suspend(value)))
    })
}

/// Reserves the state-machine bodies and their trampolines. They come right after the IR
/// functions and before the runtime, so the runtime's function indices do not move.
pub(crate) fn reserve(b: &mut Builder<'_>) {
    let ids: Vec<FnId> =
        b.m.functions
            .iter()
            .filter(|(_, f)| is_machine(f))
            .map(|(id, _)| id)
            .collect();
    let mut machines = Vec::with_capacity(ids.len());
    for id in ids {
        let (params, ret) = {
            let f = &b.m.functions[id];
            let params: Vec<ValType> = f.params.iter().map(|&p| val(f.vars[p].repr)).collect();
            (params, b.repr(f.ret))
        };
        let mut machine_params = params;
        machine_params.push(ValType::I32);
        let results: Vec<ValType> = repr_val(ret).into_iter().collect();
        let ty = b.ty(machine_params, results);
        let body = b.imports.len() as u32 + b.defs.len() as u32;
        b.defs.push(ty);
        let ty = b.ty(vec![ValType::I32], Vec::new());
        let trampoline = b.imports.len() as u32 + b.defs.len() as u32;
        b.defs.push(ty);
        let table = b.table_funcs.len() as u32;
        b.table_funcs.push(trampoline);
        machines.push(MachinePlan { id, body, table });
    }
    if let Some(plan) = &mut b.sus {
        plan.machines = machines;
    } else {
        // `prepare` runs after this; remember the machines for it.
        b.sus = Some(Plan {
            chain_head: 0,
            chain_tail: 0,
            in_mode: 0,
            in_word: 0,
            out_mode: 0,
            out_word: 0,
            pending: 0,
            sleep_op: 0,
            queue: 0,
            qhead: 0,
            qlen: 0,
            sync_depth: 0,
            perform_desc: 0,
            resume_desc: 0,
            interrupted: 0,
            twice_prefix: 0,
            after_prefix: 0,
            queue_full: 0,
            resume_table: [0; 5],
            machines,
        });
    }
}

/// Allocates the runtime's statics and descriptors and appends the resume functions to the
/// closure table. Runs after `runtime::register` (their indices are known) and before any body.
pub(crate) fn prepare(b: &mut Builder<'_>) {
    let cells = [
        b.alloc_static(4), // chain head
        b.alloc_static(4), // chain tail
        b.alloc_static(4), // input mode
        b.alloc_static(8), // input word
        b.alloc_static(4), // output mode
        b.alloc_static(8), // output word
        b.alloc_static(4), // the outermost perform of the current suspension cascade
        b.alloc_static(4), // the innermost active perform record
        b.alloc_static(QUEUE_ENTRIES * QUEUE_STRIDE),
        b.alloc_static(4), // queue head
        b.alloc_static(4), // queue length
        b.alloc_static(4), // sync depth
    ];
    let perform_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "perform".to_owned(),
        show: None,
        // used, phase, where, now mode, now word, caller chain, has caller, sync, impl chain,
        // next outer operation
        refs: vec![
            false, false, true, false, false, true, false, false, true, false,
        ],
    });
    let resume_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "resume".to_owned(),
        show: None,
        refs: vec![false, true],
    });
    let interrupted_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "interrupted".to_owned(),
        show: None,
        refs: Vec::new(),
    });
    let interrupted = b.nullary(interrupted_desc);
    let twice_prefix = b.string_literal(b"resume called twice in ");
    let after_prefix = b.string_literal(b" failed after resume: ");
    let queue_full = b.string_literal(b"internal error: the WASM scheduler queue overflowed");
    let mut resume_table = [0u32; 5];
    for (i, name) in [
        "resume_void",
        "resume_i32",
        "resume_i64",
        "resume_f64",
        "resume_ref",
    ]
    .into_iter()
    .enumerate()
    {
        let index = b.rt(name);
        let table = b.table_funcs.len() as u32;
        b.table_funcs.push(index);
        resume_table[i] = table;
    }
    let Some(plan) = &mut b.sus else { return };
    plan.chain_head = cells[0];
    plan.chain_tail = cells[1];
    plan.in_mode = cells[2];
    plan.in_word = cells[3];
    plan.out_mode = cells[4];
    plan.out_word = cells[5];
    plan.pending = cells[6];
    plan.sleep_op = cells[7];
    plan.queue = cells[8];
    plan.qhead = cells[9];
    plan.qlen = cells[10];
    plan.sync_depth = cells[11];
    plan.perform_desc = perform_desc;
    plan.resume_desc = resume_desc;
    plan.interrupted = interrupted;
    plan.twice_prefix = twice_prefix;
    plan.after_prefix = after_prefix;
    plan.queue_full = queue_full;
    plan.resume_table = resume_table;
}

/// The number of jobs the continuation queue holds. Without fibers or a host callback at most
/// one continuation waits at a time; the queue is the FIFO any future wakeups go through. The
/// scheduler indexes the same static (`runtime::sched`), so both use these constants.
pub(crate) const QUEUE_ENTRIES: u32 = 64;
/// The bytes of one job (`{chain: i32, mode: i32, word: i64}`).
pub(crate) const QUEUE_STRIDE: u32 = 16;

/// The most states one function's dispatch may have. A state is a nested block in the emitted
/// dispatch, and WebAssembly engines take quadratic time and memory to compile very deep nesting
/// (about 1 s at 500 states and 13 s at 1000 in `wasmtime` 49), so the target refuses machines
/// beyond this instead of letting one function exhaust the compiler.
const MAX_STATES: u32 = 1024;

// ---- frames ----

/// The `(state, saved)` of each resume point of a function, for the frame restore.
struct ResumePoint {
    state: u32,
    saved: Vec<VarId>,
}

/// A chunk of state-machine code, with the positions where a transition's branch goes.
#[derive(Default)]
struct Chunk {
    bytes: Vec<u8>,
    sites: Vec<usize>,
}

/// Where a `yield` of a sequence goes.
#[derive(Clone, Copy)]
enum YieldTo {
    Body,
    Stmt { dst: Option<VarId>, state: u32 },
    Landing,
}

enum MLabel {
    Block {
        label: LabelId,
        dst: Option<VarId>,
        join: u32,
    },
    Loop {
        vars: Vec<VarId>,
        body: u32,
    },
}

struct Mcx<'a, 'm> {
    b: &'a mut Builder<'m>,
    id: FnId,
    f: &'a Function,
    cx: FnCtx<'a>,
    /// The local of the dispatching state.
    state: u32,
    /// The local telling a resume point that the input is there.
    pending: u32,
    /// The frame pointer parameter.
    fr: u32,
    chunks: Vec<Option<Chunk>>,
    /// The state whose chunk is being filled (`u32::MAX` between chunks).
    cur: u32,
    labels: Vec<MLabel>,
    resumes: Vec<ResumePoint>,
}

const NO_CHUNK: u32 = u32::MAX;

fn reserve_state(mcx: &mut Mcx<'_, '_>) -> u32 {
    mcx.chunks.push(None);
    (mcx.chunks.len() - 1) as u32
}

/// Stores the chunk being filled and leaves no current chunk.
fn flush(mcx: &mut Mcx<'_, '_>, buf: &mut Chunk) {
    let done = std::mem::take(buf);
    let slot = &mut mcx.chunks[mcx.cur as usize];
    assert!(slot.is_none(), "a state-machine chunk was filled twice");
    *slot = Some(done);
    mcx.cur = NO_CHUNK;
}

/// Starts filling a reserved chunk (the previous chunk must have been flushed).
fn start(mcx: &mut Mcx<'_, '_>, buf: &mut Chunk, next: u32) {
    debug_assert!(buf.bytes.is_empty() && buf.sites.is_empty());
    debug_assert_eq!(mcx.cur, NO_CHUNK);
    debug_assert!(mcx.chunks[next as usize].is_none());
    mcx.cur = next;
}

/// Ends the current chunk with a transition to `state`.
fn goto(mcx: &mut Mcx<'_, '_>, buf: &mut Chunk, state: u32) {
    {
        let mut ins = InstructionSink::new(&mut buf.bytes);
        ins.i32_const(state as i32);
        ins.local_set(mcx.state);
        buf.sites.push(buf.bytes.len());
    }
    flush(mcx, buf);
}

fn terminate(mcx: &mut Mcx<'_, '_>, buf: &mut Chunk) {
    if mcx.cur != NO_CHUNK {
        flush(mcx, buf);
    }
}

impl<'a, 'm> Mcx<'a, 'm> {
    fn local(&self, v: VarId) -> u32 {
        self.cx.local(v)
    }

    fn operand(&mut self, ins: &mut InstructionSink, o: &Operand) {
        self.b.operand(&mut self.cx, ins, o);
    }
}

/// Emits the body and the trampoline of every state machine, in reservation order.
pub(crate) fn emit_machines(b: &mut Builder<'_>) {
    let Some(plan) = &b.sus else { return };
    let machines = plan.machines.clone();
    for mp in machines {
        let (body, trampoline) = emit_machine(b, mp);
        b.bodies.push(body);
        b.bodies.push(trampoline);
    }
}

fn emit_machine(b: &mut Builder<'_>, mp: MachinePlan) -> (WasmFunction, WasmFunction) {
    let f = b.m.functions[mp.id].clone();
    let nparams = f.params.len() as u32;
    let mut cx = FnCtx::with_extra_params(&f, b, 1);
    let declared: u32 = cx.decls.iter().map(|&(n, _)| n).sum();
    let state = nparams + 1 + declared;
    let pending = state + 1;
    cx.decls.push((2, ValType::I32));
    let mut mcx = Mcx {
        b,
        id: mp.id,
        f: &f,
        cx,
        state,
        pending,
        fr: nparams,
        chunks: Vec::new(),
        cur: NO_CHUNK,
        labels: Vec::new(),
        resumes: Vec::new(),
    };
    let entry = reserve_state(&mut mcx);
    let mut buf = Chunk::default();
    start(&mut mcx, &mut buf, entry);
    // The entry chunk: the body of the function.
    compile_seq(
        &mut mcx,
        &mut buf,
        f.body.expect("a state machine with a body"),
        YieldTo::Body,
    );
    terminate(&mut mcx, &mut buf);
    let n = mcx.chunks.len() as u32 - 1;
    if n + 1 > MAX_STATES {
        // One block per state makes the dispatch quadratic to compile in a WebAssembly engine,
        // so a function with a pathological number of suspension points is refused instead of
        // taking minutes and gigabytes.
        let name = check::display_name(&f.name);
        mcx.b.errors.push(WasmError::new(
            WasmErrorKind::Unsupported,
            format!(
                "the function `{name}` needs {} states for its suspension points, which target \
                 `wasm` cannot compile (the limit is {MAX_STATES})",
                n + 1
            ),
        ));
        return (
            WasmFunction::new(Vec::<(u32, ValType)>::new()),
            WasmFunction::new(Vec::<(u32, ValType)>::new()),
        );
    }
    debug_assert!(mcx.chunks.iter().all(Option::is_some));
    let resumes = std::mem::take(&mut mcx.resumes);
    let decls = mcx.cx.decls.clone();
    let mut func = WasmFunction::new(decls);
    {
        let mut ins = func.instructions();
        emit_restore(&mut mcx, &mut ins, &resumes);
        ins.loop_(BlockType::Empty);
        for _ in 0..=n {
            ins.block(BlockType::Empty);
        }
        ins.block(BlockType::Empty);
        ins.local_get(state);
        ins.br_table((1..=n + 1).collect::<Vec<u32>>(), 0);
        ins.end();
        ins.unreachable();
    }
    for (k, chunk) in mcx.chunks.drain(..).enumerate() {
        {
            let mut ins = func.instructions();
            ins.end();
        }
        let chunk = chunk.expect("every state-machine chunk is filled");
        append_chunk(&mut func, &chunk, n, k as u32);
    }
    {
        let mut ins = func.instructions();
        ins.end();
        ins.unreachable();
        ins.end();
    }
    // The trampoline: enter with the frame and leave the result (or the pending failure or
    // defect) where the driver picks it up.
    let trampoline = emit_trampoline(b, &f, mp);
    (func, trampoline)
}

/// The frame entry of a state machine: restore the saved variables of the frame's state, free the
/// frame and dispatch to the resume point.
fn emit_restore(mcx: &mut Mcx<'_, '_>, ins: &mut InstructionSink, resumes: &[ResumePoint]) {
    let fr = mcx.fr;
    ins.local_get(fr);
    ins.if_(BlockType::Empty);
    ins.local_get(fr);
    ins.i32_load(mem(HDR + 8, 4));
    ins.local_set(mcx.state);
    ins.i32_const(1);
    ins.local_set(mcx.pending);
    for r in resumes {
        ins.local_get(mcx.state);
        ins.i32_const(r.state as i32);
        ins.i32_eq();
        ins.if_(BlockType::Empty);
        for (i, &v) in r.saved.iter().enumerate() {
            let off = HDR + (3 + i as u32) * 8;
            let repr = mcx.f.vars[v].repr;
            // A variable without representation is never live, so it is never saved.
            debug_assert_ne!(repr, Repr::Void, "a saved variable without representation");
            if repr == Repr::Void {
                continue;
            }
            ins.local_get(fr);
            match repr {
                Repr::I32 | Repr::Ref => ins.i32_load(mem(off, 4)),
                Repr::I64 => ins.i64_load(mem(off, 8)),
                Repr::F64 => ins.f64_load(mem(off, 8)),
                Repr::Void => continue,
            };
            ins.local_set(mcx.local(v));
            if repr == Repr::Ref {
                // The reference moved from the frame into the local.
                ins.local_get(fr);
                ins.i32_const(0);
                ins.i32_store(mem(off, 4));
            }
        }
        ins.end();
    }
    ins.local_get(fr);
    ins.i32_const(0);
    ins.i32_store(mem(HDR + 16, 4));
    let drop = mcx.b.rt("drop");
    ins.local_get(fr);
    ins.call(drop);
    ins.else_();
    ins.i32_const(0);
    ins.local_set(mcx.state);
    ins.i32_const(0);
    ins.local_set(mcx.pending);
    ins.end();
}

/// Appends a chunk, writing the branches that the transitions recorded before the body's state
/// count was known. A chunk of state `k` sits inside `n - k` dispatch blocks and the loop.
fn append_chunk(func: &mut WasmFunction, chunk: &Chunk, n: u32, k: u32) {
    let depth = n - k;
    let mut pos = 0;
    for &site in &chunk.sites {
        func.raw(chunk.bytes[pos..site].iter().copied());
        let mut br = Vec::new();
        InstructionSink::new(&mut br).br(depth);
        func.raw(br);
        pos = site;
    }
    func.raw(chunk.bytes[pos..].iter().copied());
}

fn emit_trampoline(b: &mut Builder<'_>, f: &Function, mp: MachinePlan) -> WasmFunction {
    let ret = b.repr(f.ret);
    let (out_mode, out_word) = match &b.sus {
        Some(plan) => (plan.out_mode, plan.out_word),
        None => (0, 0),
    };
    let mut locals = Vec::<(u32, ValType)>::new();
    if ret != Repr::Void {
        locals.push((1, val(ret)));
    }
    let mut func = WasmFunction::new(locals);
    {
        let mut ins = func.instructions();
        // The zero arguments only fill the signature: the resume point restores every parameter
        // the edge saved from the frame (the ones it did not save are not live after it).
        for &p in &f.params {
            match f.vars[p].repr {
                Repr::I32 | Repr::Ref => {
                    ins.i32_const(0);
                }
                Repr::I64 => {
                    ins.i64_const(0);
                }
                Repr::F64 => {
                    ins.f64_const(0.0.into());
                }
                Repr::Void => {}
            }
        }
        ins.local_get(0);
        ins.call(mp.body);
        if ret != Repr::Void {
            ins.local_set(1);
            ins.i32_const(out_word as i32);
            ins.local_get(1);
            match ret {
                Repr::I32 | Repr::Ref => {
                    ins.i32_store(mem(0, 4));
                }
                Repr::I64 => {
                    ins.i64_store(mem(0, 8));
                }
                Repr::F64 => {
                    ins.f64_store(mem(0, 8));
                }
                Repr::Void => unreachable!(),
            }
        }
        // A suspension leaves the captured frames to the driver; otherwise the result (or the
        // pending failure or defect) is the input of the next frame.
        ins.global_get(G_SUSPENDING);
        ins.if_(BlockType::Empty);
        ins.i32_const(out_mode as i32);
        ins.i32_const(3);
        ins.i32_store(mem(0, 4));
        ins.return_();
        ins.end();
        ins.global_get(G_DEFECT);
        ins.if_(BlockType::Empty);
        ins.i32_const(out_mode as i32);
        ins.i32_const(2);
        ins.i32_store(mem(0, 4));
        ins.i32_const(out_word as i32);
        ins.global_get(G_DEFECT);
        ins.i32_store(mem(0, 4));
        ins.i32_const(0);
        ins.global_set(G_DEFECT);
        ins.return_();
        ins.end();
        ins.global_get(G_FAILURE);
        ins.if_(BlockType::Empty);
        ins.i32_const(out_mode as i32);
        ins.i32_const(1);
        ins.i32_store(mem(0, 4));
        ins.i32_const(out_word as i32);
        ins.global_get(G_FAILURE);
        ins.i32_store(mem(0, 4));
        ins.i32_const(0);
        ins.global_set(G_FAILURE);
        ins.return_();
        ins.end();
        ins.i32_const(out_mode as i32);
        ins.i32_const(0);
        ins.i32_store(mem(0, 4));
        ins.end();
    }
    func
}

fn compile_seq(mcx: &mut Mcx<'_, '_>, buf: &mut Chunk, id: SeqId, y: YieldTo) {
    let stmts = mcx.f.seqs[id].stmts.clone();
    for s in stmts {
        compile_stmt(mcx, buf, s);
    }
    let term = mcx.f.seqs[id].term.clone();
    compile_term(mcx, buf, &term, y);
}

fn compile_term(mcx: &mut Mcx<'_, '_>, buf: &mut Chunk, term: &Term, y: YieldTo) {
    match term {
        Term::Yield(v) => match y {
            YieldTo::Stmt { dst, state } => {
                if let (Some(d), Some(v)) = (dst, v) {
                    let mut ins = InstructionSink::new(&mut buf.bytes);
                    mcx.operand(&mut ins, v);
                    ins.local_set(mcx.local(d));
                }
                goto(mcx, buf, state);
            }
            YieldTo::Body => {
                mcx.b
                    .internal(format!("yield at the end of {}", mcx.f.name));
                let mut ins = InstructionSink::new(&mut buf.bytes);
                ins.unreachable();
                terminate(mcx, buf);
            }
            YieldTo::Landing => {
                mcx.b
                    .internal(format!("yield in a landing in {}", mcx.f.name));
                let mut ins = InstructionSink::new(&mut buf.bytes);
                ins.unreachable();
                terminate(mcx, buf);
            }
        },
        Term::Return(v) => {
            let mut ins = InstructionSink::new(&mut buf.bytes);
            if let Some(v) = v {
                mcx.operand(&mut ins, v);
            }
            ins.return_();
            terminate(mcx, buf);
        }
        Term::Fail(v) => {
            let mut ins = InstructionSink::new(&mut buf.bytes);
            mcx.operand(&mut ins, v);
            ins.global_set(G_FAILURE);
            dummy_return(&mut ins, mcx.cx.ret);
            terminate(mcx, buf);
        }
        Term::Break { label, value } => {
            let Some((join, dst)) = find_block(mcx, *label) else {
                mcx.b
                    .internal(format!("break to an unknown label in {}", mcx.f.name));
                return;
            };
            {
                let mut ins = InstructionSink::new(&mut buf.bytes);
                if let (Some(d), Some(v)) = (dst, value) {
                    mcx.operand(&mut ins, v);
                    ins.local_set(mcx.local(d));
                }
            }
            goto(mcx, buf, join);
        }
        Term::Recur(args) => {
            let Some((body, vars)) = find_loop(mcx) else {
                mcx.b
                    .internal(format!("recur outside a loop in {}", mcx.f.name));
                return;
            };
            {
                let mut ins = InstructionSink::new(&mut buf.bytes);
                for (i, (o, &v)) in args.iter().zip(&vars).enumerate() {
                    mcx.operand(&mut ins, o);
                    let r = mcx.f.vars[v].repr;
                    ins.local_set(mcx.cx.scratch(r, i));
                }
                for (i, &v) in vars.iter().enumerate() {
                    let r = mcx.f.vars[v].repr;
                    ins.local_get(mcx.cx.scratch(r, i));
                    ins.local_set(mcx.local(v));
                }
            }
            goto(mcx, buf, body);
        }
        Term::Unwind => {
            let mut ins = InstructionSink::new(&mut buf.bytes);
            dummy_return(&mut ins, mcx.cx.ret);
            terminate(mcx, buf);
        }
        Term::Unreachable => {
            let mut ins = InstructionSink::new(&mut buf.bytes);
            ins.unreachable();
            terminate(mcx, buf);
        }
        Term::Defect(..) => {
            mcx.b.internal(format!("defect left in {}", mcx.f.name));
            let mut ins = InstructionSink::new(&mut buf.bytes);
            ins.unreachable();
            terminate(mcx, buf);
        }
    }
}

fn find_block(mcx: &Mcx<'_, '_>, label: LabelId) -> Option<(u32, Option<VarId>)> {
    mcx.labels.iter().rev().find_map(|l| match l {
        MLabel::Block {
            label: id,
            dst,
            join,
        } if *id == label => Some((*join, *dst)),
        _ => None,
    })
}

fn find_loop(mcx: &Mcx<'_, '_>) -> Option<(u32, Vec<VarId>)> {
    mcx.labels.iter().rev().find_map(|l| match l {
        MLabel::Loop { vars, body } => Some((*body, vars.clone())),
        _ => None,
    })
}

fn compile_stmt(mcx: &mut Mcx<'_, '_>, buf: &mut Chunk, id: StmtId) {
    let stmt = mcx.f.stmts[id].clone();
    match stmt {
        Stmt::Let {
            dst,
            value,
            on_defect,
            on_suspend,
        } => {
            // A fairness yield has nobody to hand a turn to here (no fibers), so neither its
            // suspension edge nor its defect landing can be taken: no state, no landing copy.
            if matches!(
                &value,
                Value::Runtime {
                    op: RuntimeOp::Yield | RuntimeOp::YieldPerform,
                    ..
                }
            ) {
                debug_assert!(dst.is_none(), "a fairness yield has no destination");
                return;
            }
            let fail = match &value {
                Value::Call {
                    on_fail: Some(OnFail::Handle { error, body }),
                    ..
                }
                | Value::Fiber {
                    on_fail: Some(OnFail::Handle { error, body }),
                    ..
                } => Some((*error, *body)),
                _ => None,
            };
            let sync = !mcx.f.suspends && emela_lir::may_suspend(&value);
            {
                let mut ins = InstructionSink::new(&mut buf.bytes);
                if sync {
                    let f = mcx.b.rt("sync_enter");
                    ins.call(f);
                }
                mcx.b.value(&mut mcx.cx, &mut ins, dst, &value);
                if sync {
                    let f = mcx.b.rt("sync_leave");
                    ins.call(f);
                }
            }
            if let Some(edge) = on_suspend {
                let repr = value_repr(mcx.b, &mcx.cx, &value);
                compile_edge(mcx, buf, dst, repr, &edge);
            }
            compile_landings(mcx, buf, on_defect, fail);
        }
        Stmt::If {
            dst,
            cond,
            then,
            else_,
        } => {
            let then_state = reserve_state(mcx);
            let else_state = reserve_state(mcx);
            let join = reserve_state(mcx);
            {
                let mut ins = InstructionSink::new(&mut buf.bytes);
                mcx.operand(&mut ins, &cond);
                ins.if_(BlockType::Result(ValType::I32));
                ins.i32_const(then_state as i32);
                ins.else_();
                ins.i32_const(else_state as i32);
                ins.end();
                ins.local_set(mcx.state);
                buf.sites.push(buf.bytes.len());
            }
            flush(mcx, buf);
            start(mcx, buf, then_state);
            compile_seq(mcx, buf, then, YieldTo::Stmt { dst, state: join });
            start(mcx, buf, else_state);
            compile_seq(mcx, buf, else_, YieldTo::Stmt { dst, state: join });
            start(mcx, buf, join);
        }
        Stmt::Switch {
            dst,
            on,
            cases,
            default,
        } => {
            let case_states: Vec<u32> = cases.iter().map(|_| reserve_state(mcx)).collect();
            let default_state = default.map(|_| reserve_state(mcx));
            let join = reserve_state(mcx);
            {
                let mut ins = InstructionSink::new(&mut buf.bytes);
                emit_switch(
                    mcx.b,
                    &mut mcx.cx,
                    &mut ins,
                    on,
                    &cases,
                    &case_states,
                    default_state,
                );
                ins.local_set(mcx.state);
                buf.sites.push(buf.bytes.len());
            }
            flush(mcx, buf);
            for (case, state) in cases.iter().zip(&case_states) {
                start(mcx, buf, *state);
                compile_seq(mcx, buf, case.body, YieldTo::Stmt { dst, state: join });
            }
            if let (Some(d), Some(state)) = (default, default_state) {
                start(mcx, buf, state);
                compile_seq(mcx, buf, d, YieldTo::Stmt { dst, state: join });
            }
            start(mcx, buf, join);
        }
        Stmt::Loop {
            dst,
            vars,
            inits,
            body,
        } => {
            {
                let mut ins = InstructionSink::new(&mut buf.bytes);
                for (&v, init) in vars.iter().zip(&inits) {
                    mcx.operand(&mut ins, init);
                    ins.local_set(mcx.local(v));
                }
            }
            let body_state = reserve_state(mcx);
            let join = reserve_state(mcx);
            goto(mcx, buf, body_state);
            start(mcx, buf, body_state);
            mcx.labels.push(MLabel::Loop {
                vars: vars.clone(),
                body: body_state,
            });
            compile_seq(mcx, buf, body, YieldTo::Stmt { dst, state: join });
            mcx.labels.pop();
            start(mcx, buf, join);
        }
        Stmt::Block { dst, label, body } => {
            let body_state = reserve_state(mcx);
            let join = reserve_state(mcx);
            goto(mcx, buf, body_state);
            start(mcx, buf, body_state);
            mcx.labels.push(MLabel::Block { label, dst, join });
            compile_seq(mcx, buf, body, YieldTo::Stmt { dst, state: join });
            mcx.labels.pop();
            start(mcx, buf, join);
        }
        Stmt::Protect { .. } => {
            mcx.b.internal(format!("protect left in {}", mcx.f.name));
        }
        Stmt::Dup(v) => {
            let mut ins = InstructionSink::new(&mut buf.bytes);
            ins.local_get(mcx.local(v));
            let dup = mcx.b.rt("dup");
            ins.call(dup);
        }
        Stmt::Drop(v) => {
            let mut ins = InstructionSink::new(&mut buf.bytes);
            ins.local_get(mcx.local(v));
            let drop = mcx.b.rt("drop");
            ins.call(drop);
        }
    }
}

fn emit_switch(
    b: &mut Builder<'_>,
    cx: &mut FnCtx<'_>,
    ins: &mut InstructionSink,
    on: SwitchOn,
    cases: &[emela_lir::Case],
    states: &[u32],
    default: Option<u32>,
) {
    for (case, state) in cases.iter().zip(states) {
        match on {
            SwitchOn::Tag(v) => {
                if cx.f.vars[v].repr == Repr::Ref {
                    ins.local_get(cx.local(v));
                    let tag = b.rt("tag");
                    ins.call(tag);
                } else {
                    ins.local_get(cx.local(v));
                }
                match &case.key {
                    CaseKey::Variant(k) => {
                        ins.i32_const(*k as i32);
                    }
                    CaseKey::Lit(c) => b.const_val(ins, c),
                }
                ins.i32_eq();
            }
            SwitchOn::Lit(v) => {
                ins.local_get(cx.local(v));
                match &case.key {
                    CaseKey::Lit(c) => b.const_val(ins, c),
                    CaseKey::Variant(k) => {
                        ins.i32_const(*k as i32);
                    }
                }
                if cx.f.vars[v].repr == Repr::I64 {
                    ins.i64_eq();
                } else {
                    ins.i32_eq();
                }
            }
        }
        ins.if_(BlockType::Result(ValType::I32));
        ins.i32_const(*state as i32);
        ins.else_();
    }
    match default {
        Some(s) => {
            ins.i32_const(s as i32);
        }
        None => {
            ins.unreachable();
        }
    }
    for _ in cases {
        ins.end();
    }
}

/// A suspension point: when the value came back suspended, push a frame and return; the resume
/// point runs the input into `dst` and then the landings. `repr` is the value's representation,
/// which is needed when the value is unused (`dst` is `None`) and the resumed reference must be
/// released.
fn compile_edge(
    mcx: &mut Mcx<'_, '_>,
    buf: &mut Chunk,
    dst: Option<VarId>,
    repr: Repr,
    edge: &Suspend,
) {
    let resume = reserve_state(mcx);
    let mut refs = vec![false, false, true];
    refs.extend(edge.saved.iter().map(|&v| mcx.f.vars[v].repr == Repr::Ref));
    let desc = mcx.b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: check::display_name(&mcx.f.name).to_owned(),
        show: None,
        refs,
    });
    let table = mcx
        .b
        .sus
        .as_ref()
        .and_then(|p| p.machine(mcx.id))
        .map(|m| m.table)
        .unwrap_or(0);
    {
        let mut ins = InstructionSink::new(&mut buf.bytes);
        ins.global_get(G_SUSPENDING);
        ins.if_(BlockType::Empty);
        ins.i32_const(desc as i32);
        ins.i32_const(table as i32);
        // The frame stores the dispatch chunk, which is what the restore switches on.
        ins.i32_const(resume as i32);
        ins.i32_const(edge.saved.len() as i32);
        let f = mcx.b.rt("frame_new");
        ins.call(f);
        ins.local_set(mcx.cx.scratch(Repr::Ref, 0));
        for (i, &v) in edge.saved.iter().enumerate() {
            let off = HDR + (3 + i as u32) * 8;
            let repr = mcx.f.vars[v].repr;
            // A variable without representation is never live, so it is never saved.
            debug_assert_ne!(repr, Repr::Void, "a saved variable without representation");
            ins.local_get(mcx.cx.scratch(Repr::Ref, 0));
            ins.local_get(mcx.local(v));
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
        let push = mcx.b.rt("frame_push");
        ins.local_get(mcx.cx.scratch(Repr::Ref, 0));
        ins.call(push);
        dummy_return(&mut ins, mcx.cx.ret);
        ins.end();
    }
    goto(mcx, buf, resume);
    start(mcx, buf, resume);
    // The resume point: the input is the suspended value's result, or a pending failure or
    // defect that the landings below see.
    {
        let Some(plan) = &mcx.b.sus else { return };
        let (in_mode, in_word) = (plan.in_mode, plan.in_word);
        let mut ins = InstructionSink::new(&mut buf.bytes);
        ins.local_get(mcx.pending);
        ins.if_(BlockType::Empty);
        ins.i32_const(in_mode as i32);
        ins.i32_load8_u(mem(0, 1));
        ins.i32_const(2);
        ins.i32_eq();
        ins.if_(BlockType::Empty);
        ins.i32_const(in_word as i32);
        ins.i32_load(mem(0, 4));
        ins.global_set(G_DEFECT);
        ins.else_();
        ins.i32_const(in_mode as i32);
        ins.i32_load8_u(mem(0, 1));
        ins.i32_const(1);
        ins.i32_eq();
        ins.if_(BlockType::Empty);
        ins.i32_const(in_word as i32);
        ins.i32_load(mem(0, 4));
        ins.global_set(G_FAILURE);
        ins.else_();
        match dst {
            Some(d) => {
                ins.i32_const(in_word as i32);
                match mcx.f.vars[d].repr {
                    Repr::I32 | Repr::Ref => {
                        ins.i32_load(mem(0, 4));
                    }
                    Repr::I64 => {
                        ins.i64_load(mem(0, 8));
                    }
                    Repr::F64 => {
                        ins.f64_load(mem(0, 8));
                    }
                    Repr::Void => {}
                }
                ins.local_set(mcx.local(d));
            }
            None if repr == Repr::Ref => {
                // The value is unused: the resumed reference is released, like the fast path
                // (`assign` with no destination) does.
                ins.i32_const(in_word as i32);
                ins.i32_load(mem(0, 4));
                let drop = mcx.b.rt("drop");
                ins.call(drop);
            }
            None => {}
        }
        ins.end();
        ins.end();
        ins.i32_const(0);
        ins.local_set(mcx.pending);
        ins.end();
    }
    mcx.resumes.push(ResumePoint {
        state: resume,
        saved: edge.saved.clone(),
    });
}

/// Whether a landing can be emitted in place: it is straight-line (no control flow of its own, so
/// it needs no state) and leaves the function. The defect landings that the Heap IR builds are
/// cleanups of this shape.
fn landing_inline(f: &Function, seq: SeqId) -> bool {
    inline_stmts(f, seq)
        && matches!(
            f.seqs[seq].term,
            Term::Unwind | Term::Unreachable | Term::Return(_) | Term::Fail(_)
        )
}

/// The statements of an in-place landing: no landing, no edge, no structured statement that would
/// need a state. A block is allowed when it simply ends (the cleanup blocks of a `with` do).
fn inline_stmts(f: &Function, seq: SeqId) -> bool {
    let s = &f.seqs[seq];
    s.stmts.iter().all(|&id| match &f.stmts[id] {
        Stmt::Let {
            value,
            on_defect: None,
            on_suspend: None,
            ..
        } => value_on_fail(value).is_none(),
        Stmt::Dup(_) | Stmt::Drop(_) => true,
        Stmt::Block { body, .. } => {
            inline_stmts(f, *body)
                && matches!(
                    f.seqs[*body].term,
                    Term::Yield(None)
                        | Term::Unwind
                        | Term::Unreachable
                        | Term::Return(_)
                        | Term::Fail(_)
                )
        }
        _ => false,
    })
}

/// Emits a landing that [`landing_inline`] accepted, without starting or ending a state.
fn compile_landing_inline(mcx: &mut Mcx<'_, '_>, buf: &mut Chunk, seq: SeqId) {
    let stmts = mcx.f.seqs[seq].stmts.clone();
    for id in stmts {
        match mcx.f.stmts[id].clone() {
            Stmt::Let { dst, value, .. } => {
                let mut ins = InstructionSink::new(&mut buf.bytes);
                mcx.b.value(&mut mcx.cx, &mut ins, dst, &value);
            }
            Stmt::Dup(v) => {
                let mut ins = InstructionSink::new(&mut buf.bytes);
                ins.local_get(mcx.local(v));
                let dup = mcx.b.rt("dup");
                ins.call(dup);
            }
            Stmt::Drop(v) => {
                let mut ins = InstructionSink::new(&mut buf.bytes);
                ins.local_get(mcx.local(v));
                let drop = mcx.b.rt("drop");
                ins.call(drop);
            }
            Stmt::Block { body, .. } => compile_landing_inline(mcx, buf, body),
            _ => {
                let name = mcx.f.name.clone();
                mcx.b
                    .internal(format!("a landing with control flow in {name}"));
                return;
            }
        }
    }
    let term = mcx.f.seqs[seq].term.clone();
    let mut ins = InstructionSink::new(&mut buf.bytes);
    match term {
        // A block that simply ends: the statements after it run next.
        Term::Yield(None) => {}
        Term::Unwind => dummy_return(&mut ins, mcx.cx.ret),
        Term::Unreachable => {
            ins.unreachable();
        }
        Term::Return(v) => {
            if let Some(v) = v {
                mcx.operand(&mut ins, &v);
            }
            ins.return_();
        }
        Term::Fail(v) => {
            mcx.operand(&mut ins, &v);
            ins.global_set(G_FAILURE);
            dummy_return(&mut ins, mcx.cx.ret);
        }
        _ => mcx.b.internal("a landing that does not return"),
    }
}

/// The defect and failure landings of a value, if any. A straight-line landing runs in place after
/// the test (no state); the others are separate chunks, and the sequence then continues in a new
/// chunk.
fn compile_landings(
    mcx: &mut Mcx<'_, '_>,
    buf: &mut Chunk,
    on_defect: Option<SeqId>,
    fail: Option<(VarId, SeqId)>,
) {
    if on_defect.is_none() && fail.is_none() {
        return;
    }
    let inline = on_defect.filter(|&seq| landing_inline(mcx.f, seq));
    let defect_state = on_defect
        .filter(|_| inline.is_none())
        .map(|_| reserve_state(mcx));
    let fail_state = fail.map(|_| reserve_state(mcx));
    let cont = if defect_state.is_some() || fail_state.is_some() {
        Some(reserve_state(mcx))
    } else {
        None
    };
    if let Some(seq) = inline {
        let mut ins = InstructionSink::new(&mut buf.bytes);
        ins.global_get(G_DEFECT);
        ins.if_(BlockType::Empty);
        compile_landing_inline(mcx, buf, seq);
        let mut ins = InstructionSink::new(&mut buf.bytes);
        ins.end();
    }
    let Some(cont) = cont else { return };
    {
        let mut ins = InstructionSink::new(&mut buf.bytes);
        if let Some(defect_state) = defect_state {
            ins.global_get(G_DEFECT);
            ins.if_(BlockType::Result(ValType::I32));
            ins.i32_const(defect_state as i32);
            ins.else_();
        }
        if let Some((error, _)) = fail {
            let fail_state = fail_state.expect("a failure state");
            ins.global_get(G_FAILURE);
            ins.if_(BlockType::Result(ValType::I32));
            // The error variable is the pending failure (the landing body sees it owned).
            ins.global_get(G_FAILURE);
            ins.local_set(mcx.local(error));
            ins.i32_const(0);
            ins.global_set(G_FAILURE);
            ins.i32_const(fail_state as i32);
            ins.else_();
            ins.i32_const(cont as i32);
            ins.end();
        } else {
            ins.i32_const(cont as i32);
        }
        if defect_state.is_some() {
            ins.end();
        }
        ins.local_set(mcx.state);
        buf.sites.push(buf.bytes.len());
    }
    flush(mcx, buf);
    if let Some(state) = defect_state {
        start(mcx, buf, state);
        compile_seq(
            mcx,
            buf,
            on_defect.expect("a defect landing"),
            YieldTo::Landing,
        );
    }
    if let Some((_, body)) = fail {
        start(mcx, buf, fail_state.expect("a failure landing"));
        compile_seq(mcx, buf, body, YieldTo::Landing);
    }
    start(mcx, buf, cont);
}
