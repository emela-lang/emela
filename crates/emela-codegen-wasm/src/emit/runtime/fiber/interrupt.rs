//! The records' intrusive lists and the delivery of interruptions.

use super::*;

// ---- records and intrusive lists ----

/// `fb_scope_of(o)`: the scope record is the Scope value itself.
pub(super) fn scope_of(_b: &mut Builder<'_>, ins: &mut InstructionSink) {
    ins.local_get(0);
}

/// `fb_fiber_of(o)`: the fiber record is the Fiber handle itself.
pub(super) fn fiber_of(_b: &mut Builder<'_>, ins: &mut InstructionSink) {
    ins.local_get(0);
}

/// The nodes of the pending and unwinding interruption lists, appended in arrival order.
pub(super) fn push_node(
    b: &mut Builder<'_>,
    ins: &mut InstructionSink,
    f: u32,
    target: u32,
    head: u32,
    tail: u32,
    n: u32,
) {
    let node: u32 = n;
    let desc = plan(b).node_desc;
    c(ins, N_SLOTS);
    c(ins, desc as i32);
    call(b, ins, "alloc");
    ins.local_set(node);
    st_const(ins, node, N_NEXT, 0);
    g(ins, target);
    ins.if_(BlockType::Empty);
    st_dup(b, ins, node, N_TARGET, target);
    ins.else_();
    st_const(ins, node, N_TARGET, 0);
    ins.end();
    ldl(ins, f, tail);
    ins.if_(BlockType::Empty);
    ldl(ins, f, tail);
    g(ins, node);
    ins.i32_store(mem(N_NEXT, 4));
    ins.else_();
    stl(ins, f, head, node);
    ins.end();
    stl(ins, f, tail, node);
}

/// `fb_pending_has(f, target) -> i32`: whether an interruption of `target` (0: the whole fiber)
/// is pending.
pub(super) fn pending_has(_b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const P: u32 = 2;
    ldl(ins, 0, F_PHEAD);
    ins.local_set(P);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, P);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, P, N_TARGET);
    g(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    c(ins, 1);
    ins.return_();
    ins.end();
    ldl(ins, P, N_NEXT);
    ins.local_set(P);
    ins.br(0);
    ins.end();
    ins.end();
    c(ins, 0);
}

/// `fb_pending_push(f, target)`: add an interruption to the pending list (target is borrowed).
pub(super) fn pending_push(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    push_node(b, ins, 0, 1, F_PHEAD, F_PTAIL, 4);
}

/// `fb_unwinds_has(f, target) -> i32`: whether `target` is in the unwinding list.
pub(super) fn unwinds_has(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const P: u32 = 2;
    ldl(ins, 0, F_UHEAD);
    ins.local_set(P);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, P);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, P, N_TARGET);
    g(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    c(ins, 1);
    ins.return_();
    ins.end();
    ldl(ins, P, N_NEXT);
    ins.local_set(P);
    ins.br(0);
    ins.end();
    ins.end();
    c(ins, 0);
    let _ = b;
}

/// `fb_pending_clear(f)`: drop every pending interruption and every scope that unwinds.
pub(super) fn pending_clear(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const N: u32 = 2;
    const NEXT: u32 = 3;
    for head in [F_PHEAD, F_UHEAD] {
        ldl(ins, 0, head);
        ins.local_set(N);
        ins.block(BlockType::Empty);
        ins.loop_(BlockType::Empty);
        g(ins, N);
        ins.i32_eqz();
        ins.br_if(1);
        ldl(ins, N, N_NEXT);
        ins.local_set(NEXT);
        dropv(b, ins, N);
        g(ins, NEXT);
        ins.local_set(N);
        ins.br(0);
        ins.end();
        ins.end();
        st_const(ins, 0, head, 0);
    }
    st_const(ins, 0, F_PTAIL, 0);
    st_const(ins, 0, F_UTAIL, 0);
}

/// `fb_pending_remove(f, target)`: remove every node of `target` from the pending and the
/// unwinding lists.
pub(super) fn pending_remove(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const N: u32 = 2;
    const NEXT: u32 = 3;
    const PREV: u32 = 4;
    for (head, tail) in [(F_PHEAD, F_PTAIL), (F_UHEAD, F_UTAIL)] {
        c(ins, 0);
        ins.local_set(PREV);
        ldl(ins, 0, head);
        ins.local_set(N);
        ins.block(BlockType::Empty);
        ins.loop_(BlockType::Empty);
        g(ins, N);
        ins.i32_eqz();
        ins.br_if(1);
        ldl(ins, N, N_NEXT);
        ins.local_set(NEXT);
        ldl(ins, N, N_TARGET);
        g(ins, 1);
        ins.i32_eq();
        ins.if_(BlockType::Empty);
        g(ins, PREV);
        ins.if_(BlockType::Empty);
        stl(ins, PREV, N_NEXT, NEXT);
        ins.else_();
        stl(ins, 0, head, NEXT);
        ins.end();
        g(ins, NEXT);
        ins.if_(BlockType::Empty);
        ins.else_();
        stl(ins, 0, tail, PREV);
        ins.end();
        dropv(b, ins, N);
        ins.else_();
        g(ins, N);
        ins.local_set(PREV);
        ins.end();
        g(ins, NEXT);
        ins.local_set(N);
        ins.br(0);
        ins.end();
        ins.end();
    }
}

/// `fb_interrupting(f) -> i32`: whether an interruption is ready to be delivered (spec 11.4: not
/// during a release).
pub(super) fn interrupting(_b: &mut Builder<'_>, ins: &mut InstructionSink) {
    ldl(ins, 0, F_HOLDS);
    ins.if_(BlockType::Empty);
    c(ins, 0);
    ins.return_();
    ins.end();
    ldl(ins, 0, F_FLAGS);
    c(ins, FF_UNWINDING);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    c(ins, 1);
    ins.return_();
    ins.end();
    ldl(ins, 0, F_PHEAD);
    ins.if_(BlockType::Empty);
    c(ins, 1);
    ins.return_();
    ins.end();
    ldl(ins, 0, F_UHEAD);
    ins.i32_eqz();
    ins.i32_eqz();
}

/// `fb_is_unwinding(f) -> i32`: whether the fiber is already unwinding as interrupted.
pub(super) fn is_unwinding(_b: &mut Builder<'_>, ins: &mut InstructionSink) {
    ldl(ins, 0, F_FLAGS);
    c(ins, FF_UNWINDING);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    c(ins, 1);
    ins.return_();
    ins.end();
    ldl(ins, 0, F_UHEAD);
    ins.i32_eqz();
    ins.i32_eqz();
}

/// `fb_interrupted_new(scope) -> Ref`: an interruption of `scope`'s body (0: the whole fiber).
pub(super) fn interrupted_new(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const I: u32 = 1;
    let desc = plan(b).interrupted_desc;
    c(ins, 1);
    c(ins, desc as i32);
    call(b, ins, "alloc");
    ins.local_set(I);
    g(ins, 0);
    ins.if_(BlockType::Empty);
    st_dup(b, ins, I, HDR, 0);
    ins.else_();
    st_const(ins, I, HDR, 0);
    ins.end();
    g(ins, I);
}

/// Pushes `scope` and calls `fb_interrupted_new`.
pub(super) fn new_interruption(b: &mut Builder<'_>, ins: &mut InstructionSink, scope: u32) {
    g(ins, scope);
    call(b, ins, "fb_interrupted_new");
}

/// `fb_is_interrupted(x) -> i32`: whether `x` is an interruption. A scope's interruption is a
/// fresh object, so the descriptor decides, not the pointer.
pub(super) fn is_interrupted(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    let desc = plan(b).interrupted_desc;
    g(ins, 0);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    c(ins, 0);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(8, 4));
    c(ins, desc as i32);
    ins.i32_eq();
}

/// `fb_deliver(f) -> Ref`: take the outermost pending interruption and return it as the defect.
pub(super) fn deliver(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const TARGET: u32 = 1;
    const N: u32 = 2;
    const BEST: u32 = 3;
    const MIN: u32 = 4;
    const NEXT: u32 = 5;
    const F: u32 = 6;
    // A whole-fiber interruption overrides everything.
    ldl(ins, 0, F_FLAGS);
    c(ins, FF_UNWINDING);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    c(ins, 0);
    ins.local_set(TARGET);
    ins.else_();
    g(ins, 0);
    c(ins, 0);
    call(b, ins, "fb_pending_has");
    ins.if_(BlockType::Empty);
    c(ins, 0);
    ins.local_set(TARGET);
    ins.else_();
    // The outermost scope of the pending and unwinding lists (the smallest depth).
    c(ins, 0);
    ins.local_set(MIN);
    c(ins, 0);
    ins.local_set(TARGET);
    for head in [F_PHEAD, F_UHEAD] {
        ldl(ins, 0, head);
        ins.local_set(N);
        ins.block(BlockType::Empty);
        ins.loop_(BlockType::Empty);
        g(ins, N);
        ins.i32_eqz();
        ins.br_if(1);
        ldl(ins, N, N_TARGET);
        ins.local_set(BEST);
        g(ins, BEST);
        ins.if_(BlockType::Empty);
        g(ins, TARGET);
        ins.i32_eqz();
        ldl(ins, BEST, S_DEPTH);
        g(ins, MIN);
        ins.i32_lt_s();
        ins.i32_or();
        ins.if_(BlockType::Empty);
        g(ins, BEST);
        ins.local_set(TARGET);
        ldl(ins, BEST, S_DEPTH);
        ins.local_set(MIN);
        ins.end();
        ins.end();
        ldl(ins, N, N_NEXT);
        ins.local_set(NEXT);
        g(ins, NEXT);
        ins.local_set(N);
        ins.br(0);
        ins.end();
        ins.end();
    }
    ins.end();
    ins.end();
    g(ins, TARGET);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    // The whole fiber: forget every narrower interruption and remember that it unwinds.
    ldl(ins, 0, F_FLAGS);
    c(ins, FF_UNWINDING);
    ins.i32_or();
    ins.local_set(F);
    stl(ins, 0, F_FLAGS, F);
    g(ins, 0);
    call(b, ins, "fb_pending_clear");
    c(ins, 0);
    ins.local_set(TARGET);
    ins.else_();
    // A scope's body unwinds: it moves from pending to unwinding, so the scope boundary can
    // clear it when the body is done.
    g(ins, 0);
    g(ins, TARGET);
    call(b, ins, "fb_pending_remove");
    push_node(b, ins, 0, TARGET, F_UHEAD, F_UTAIL, 6);
    ins.end();
    new_interruption(b, ins, TARGET);
}
