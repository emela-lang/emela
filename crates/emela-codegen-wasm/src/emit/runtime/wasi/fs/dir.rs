//! Directories: making one, and the sorted names of the entries of one.

use super::*;

/// `fs_mkdir(root, ptr, len) -> i32`: creates one directory; `exist` is not an error.
pub(super) fn mkdir(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const ST: u32 = 3;
    let p = plan(b);
    let create = import(
        b,
        "[async-lower][method]descriptor.create-directory-at",
        vec![ValType::I32; 4],
        vec![ValType::I32],
    );
    let await_ = b.rt("fs_await");
    let classify = b.rt("fs_classify");
    ins.local_get(0);
    ins.local_get(1);
    ins.local_get(2);
    ins.i32_const(p.buf as i32);
    ins.call(create);
    ins.call(await_);
    ins.local_set(ST);
    ins.local_get(ST);
    ins.i32_const(2);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.i32_const(p.err as i32);
    ins.i32_const(E_FAILURE);
    ins.i32_store(mem(0, 4));
    ins.i32_const(1);
    ins.return_();
    ins.end();
    ins.i32_const(p.buf as i32);
    ins.i32_load8_u(mem(0, 1));
    ins.if_(BlockType::Empty);
    // Already there is fine for `create_directory`.
    ins.i32_const((p.buf + 4) as i32);
    ins.i32_load8_u(mem(0, 1));
    ins.i32_const(EC_EXIST);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.i32_const(p.err as i32);
    ins.i32_const((p.buf + 4) as i32);
    ins.i32_const(OP_CREATE);
    ins.call(classify);
    ins.i32_store(mem(0, 4));
    ins.i32_const(1);
    ins.return_();
    ins.end();
    ins.i32_const(0);
}

/// `fs_names_push(name)`: appends one string object to the listing.
pub(super) fn names_push(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const NEW: u32 = 1;
    const CAP: u32 = 2;
    let p = plan(b);
    let alloc = b.rt("alloc");
    // Grow when full. The first push starts with room for four names.
    ins.i32_const(p.count as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(p.cap as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_ge_u();
    ins.if_(BlockType::Empty);
    ins.i32_const(p.cap as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(1);
    ins.i32_shl();
    ins.i32_const(4);
    ins.i32_add();
    ins.local_set(CAP);
    ins.local_get(CAP);
    ins.i32_const(p.raw_desc as i32);
    ins.call(alloc);
    ins.local_set(NEW);
    ins.i32_const(p.count as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(NEW);
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.i32_const(p.names as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.i32_const(p.count as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(3);
    ins.i32_shl();
    ins.memory_copy(0, 0);
    ins.end();
    ins.i32_const(p.names as i32);
    ins.local_get(NEW);
    ins.i32_store(mem(0, 4));
    ins.i32_const(p.cap as i32);
    ins.local_get(CAP);
    ins.i32_store(mem(0, 4));
    ins.end();
    ins.i32_const(p.names as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.i32_const(p.count as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(3);
    ins.i32_shl();
    ins.i32_add();
    ins.local_get(0);
    ins.i32_store(mem(0, 4));
    ins.i32_const(p.count as i32);
    ins.i32_const(p.count as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(1);
    ins.i32_add();
    ins.i32_store(mem(0, 4));
}

/// Emits the address of name slot `index` of the last listing, plus `offset` bytes.
pub(super) fn names_slot(ins: &mut InstructionSink, p: Plan, index: u32, offset: i32) {
    ins.i32_const(p.names as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const((HDR as i32) + offset);
    ins.i32_add();
    ins.local_get(index);
    ins.i32_const(3);
    ins.i32_shl();
    ins.i32_add();
}

/// `fs_names_sort()`: heapsort of the listed names by UTF-8 byte order.
///
/// The slots hold pointers and are sorted in place, so no array is allocated. The names are
/// not repeated, so the order does not depend on stability.
pub(super) fn names_sort(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const START: u32 = 0;
    const END: u32 = 1;
    const ROOT: u32 = 2;
    const CHILD: u32 = 3;
    const A: u32 = 4;
    const B: u32 = 5;
    let p = plan(b);
    let cmp = b.rt("str_cmp");
    // Fewer than two names are already in order.
    ins.i32_const(p.count as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(2);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    // `start` walks down the parents while the max heap is built. It then stays zero while the
    // largest remaining name is swapped to the end and the rest is sifted again.
    ins.i32_const(p.count as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(1);
    ins.i32_shr_u();
    ins.local_set(START);
    ins.i32_const(p.count as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(END);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(START);
    ins.i32_const(0);
    ins.i32_gt_u();
    ins.if_(BlockType::Empty);
    ins.local_get(START);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(START);
    ins.else_();
    // One name left: the array is in order.
    ins.local_get(END);
    ins.i32_eqz();
    ins.br_if(2);
    // Move the root (the largest name) to `end` and shrink the heap.
    ins.i32_const(0);
    ins.local_set(ROOT);
    names_slot(ins, p, ROOT, 0);
    ins.i32_load(mem(0, 4));
    ins.local_set(A);
    names_slot(ins, p, END, 0);
    ins.i32_load(mem(0, 4));
    ins.local_set(B);
    names_slot(ins, p, ROOT, 0);
    ins.local_get(B);
    ins.i32_store(mem(0, 4));
    names_slot(ins, p, END, 0);
    ins.local_get(A);
    ins.i32_store(mem(0, 4));
    ins.local_get(END);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(END);
    ins.end();
    // Sift the name at `start` down until the heap holds again below `end`.
    ins.local_get(START);
    ins.local_set(ROOT);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(ROOT);
    ins.i32_const(1);
    ins.i32_shl();
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(CHILD);
    // A root without a child is a leaf.
    ins.local_get(CHILD);
    ins.local_get(END);
    ins.i32_gt_u();
    ins.br_if(1);
    // Take the right child when it exists and holds the larger name.
    ins.local_get(CHILD);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_get(END);
    ins.i32_le_u();
    ins.if_(BlockType::Empty);
    names_slot(ins, p, CHILD, 0);
    ins.i32_load(mem(0, 4));
    ins.local_set(A);
    names_slot(ins, p, CHILD, 8);
    ins.i32_load(mem(0, 4));
    ins.local_set(B);
    ins.local_get(A);
    ins.local_get(B);
    ins.call(cmp);
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.if_(BlockType::Empty);
    ins.local_get(CHILD);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(CHILD);
    ins.end();
    ins.end();
    // The heap holds when the root is not smaller than the child.
    names_slot(ins, p, ROOT, 0);
    ins.i32_load(mem(0, 4));
    ins.local_set(A);
    names_slot(ins, p, CHILD, 0);
    ins.i32_load(mem(0, 4));
    ins.local_set(B);
    ins.local_get(A);
    ins.local_get(B);
    ins.call(cmp);
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.i32_eqz();
    ins.br_if(1);
    // slot[root] = B; slot[child] = A.
    names_slot(ins, p, ROOT, 0);
    ins.local_get(B);
    ins.i32_store(mem(0, 4));
    names_slot(ins, p, CHILD, 0);
    ins.local_get(A);
    ins.i32_store(mem(0, 4));
    ins.local_get(CHILD);
    ins.local_set(ROOT);
    ins.br(0);
    ins.end();
    ins.end();
    ins.br(0);
    ins.end();
    ins.end();
}
