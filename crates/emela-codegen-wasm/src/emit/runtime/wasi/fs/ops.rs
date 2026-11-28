//! The operations of `Std.FileSystem`, each from the work directory to its outcome block.

use super::*;

/// `fs_outcome()`: the outcome block for the operation's error, message, text and names.
pub(super) fn outcome(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const O: u32 = 0;
    const NAME: u32 = 1;
    let p = plan(b);
    let alloc = b.rt("alloc");
    ins.i32_const(5);
    ins.i32_const(p.outcome_desc as i32);
    ins.call(alloc);
    ins.local_set(O);
    // The error name from the table.
    ins.i32_const(p.err_table as i32);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(2);
    ins.i32_shl();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(NAME);
    ins.local_get(O);
    ins.local_get(NAME);
    ins.i32_store(mem(OUT_ERR, 4));
    ins.local_get(O);
    ins.i32_const(p.msg as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_store(mem(OUT_MSG, 4));
    ins.local_get(O);
    ins.i32_const(p.text as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_store(mem(OUT_TEXT, 4));
    ins.local_get(O);
    ins.i32_const(p.names as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_store(mem(OUT_NAMES, 4));
    ins.local_get(O);
    ins.i32_const(p.count as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_store(mem(OUT_COUNT, 4));
    ins.local_get(O);
}

pub(super) fn read_text(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const ROOT: u32 = 1;
    const FD: u32 = 2;
    const TEXT: u32 = 3;
    let p = plan(b);
    let reset = b.rt("fs_reset");
    let root = b.rt("fs_root");
    let open = b.rt("fs_open");
    let read_all = b.rt("fs_read_all");
    let outcome = b.rt("fs_outcome");
    let drop_ = descriptor_drop(b);
    ins.call(reset);
    ins.call(root);
    ins.local_set(ROOT);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.local_get(ROOT);
    ins.local_get(0);
    ins.i32_const(0); // open-flags
    ins.i32_const(1); // descriptor-flags::read
    ins.i32_const(OP_READ);
    ins.call(open);
    ins.local_set(FD);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.local_get(FD);
    ins.call(read_all);
    ins.local_set(TEXT);
    ins.local_get(FD);
    ins.call(drop_);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.i32_const(p.text as i32);
    ins.local_get(TEXT);
    ins.i32_store(mem(0, 4));
    ins.call(outcome);
}

pub(super) fn write_text(b: &mut Builder<'_>, ins: &mut InstructionSink, append: bool) {
    const ROOT: u32 = 2;
    const FD: u32 = 3;
    let p = plan(b);
    let reset = b.rt("fs_reset");
    let root = b.rt("fs_root");
    let open = b.rt("fs_open");
    let write_all = b.rt("fs_write_all");
    let outcome = b.rt("fs_outcome");
    let drop_ = descriptor_drop(b);
    ins.call(reset);
    ins.call(root);
    ins.local_set(ROOT);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.local_get(ROOT);
    ins.local_get(0);
    // write_text truncates; append_text opens for append.
    ins.i32_const(if append { 1 } else { 9 }); // create | truncate
    ins.i32_const(2); // descriptor-flags::write
    ins.i32_const(OP_WRITE);
    ins.call(open);
    ins.local_set(FD);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.local_get(FD);
    ins.local_get(1);
    ins.i32_const(if append { 1 } else { 0 });
    ins.call(write_all);
    ins.local_get(FD);
    ins.call(drop_);
    ins.call(outcome);
}

pub(super) fn exists(b: &mut Builder<'_>, ins: &mut InstructionSink, directory: bool) {
    const ROOT: u32 = 1;
    let p = plan(b);
    let reset = b.rt("fs_reset");
    let root = b.rt("fs_root");
    let stat = b.rt("fs_stat");
    ins.call(reset);
    ins.call(root);
    ins.local_set(ROOT);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(ROOT);
    ins.local_get(0);
    ins.i32_const(1); // path-flags::symlink-follow
    ins.call(stat);
    ins.drop();
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    if directory {
        ins.i32_const(p.ty as i32);
        ins.i32_load(mem(0, 4));
        ins.i32_const(DT_DIRECTORY);
        ins.i32_eq();
    } else {
        ins.i32_const(1);
    }
}

pub(super) fn list_directory(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const ROOT: u32 = 1;
    const FD: u32 = 2;
    const STREAM: u32 = 3;
    const FUTURE: u32 = 4;
    const CODE: u32 = 5;
    const AMOUNT: u32 = 6;
    const STATUS: u32 = 7;
    const I: u32 = 8;
    const NAME_PTR: u32 = 9;
    const NAME_LEN: u32 = 10;
    let p = plan(b);
    let reset = b.rt("fs_reset");
    let root = b.rt("fs_root");
    let open = b.rt("fs_open");
    let outcome = b.rt("fs_outcome");
    let wait_code = b.rt("fs_wait_code");
    let push = b.rt("fs_names_push");
    let sort = b.rt("fs_names_sort");
    let str_from_raw = b.rt("str_from_raw");
    let classify = b.rt("fs_classify");
    let drop_ = descriptor_drop(b);
    let read_directory = import(
        b,
        "[method]descriptor.read-directory",
        vec![ValType::I32; 2],
        Vec::new(),
    );
    let stream_read = import(
        b,
        "[async-lower][stream-read-0][method]descriptor.read-directory",
        vec![ValType::I32; 3],
        vec![ValType::I32],
    );
    let future_read = import(
        b,
        "[async-lower][future-read-1][method]descriptor.read-directory",
        vec![ValType::I32; 2],
        vec![ValType::I32],
    );
    let drop_stream = import(
        b,
        "[stream-drop-readable-0][method]descriptor.read-directory",
        vec![ValType::I32],
        Vec::new(),
    );
    let drop_future = import(
        b,
        "[future-drop-readable-1][method]descriptor.read-directory",
        vec![ValType::I32],
        Vec::new(),
    );
    ins.call(reset);
    ins.call(root);
    ins.local_set(ROOT);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.local_get(ROOT);
    ins.local_get(0);
    ins.i32_const(2); // open-flags::directory
    ins.i32_const(1); // descriptor-flags::read
    ins.i32_const(OP_LIST);
    ins.call(open);
    ins.local_set(FD);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.local_get(FD);
    ins.i32_const(p.buf as i32);
    ins.call(read_directory);
    ins.i32_const(p.buf as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(STREAM);
    ins.i32_const((p.buf + 4) as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(FUTURE);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(STREAM);
    ins.i32_const(p.dirbuf as i32);
    ins.i32_const(16);
    ins.call(stream_read);
    ins.local_set(CODE);
    ins.local_get(CODE);
    ins.i32_const(-1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(STREAM);
    ins.call(wait_code);
    ins.local_set(CODE);
    ins.end();
    ins.local_get(CODE);
    ins.i32_const(4);
    ins.i32_shr_u();
    ins.local_set(AMOUNT);
    ins.local_get(CODE);
    ins.i32_const(0xF);
    ins.i32_and();
    ins.local_set(STATUS);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(AMOUNT);
    ins.i32_ge_u();
    ins.br_if(1);
    // One `directory-entry` is 24 bytes: type (16) then the name string (pointer, length).
    ins.i32_const((p.dirbuf + 16) as i32);
    ins.local_get(I);
    ins.i32_const(24);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(NAME_PTR);
    ins.i32_const((p.dirbuf + 20) as i32);
    ins.local_get(I);
    ins.i32_const(24);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(NAME_LEN);
    ins.local_get(NAME_PTR);
    ins.local_get(NAME_LEN);
    ins.call(str_from_raw);
    ins.call(push);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(STATUS);
    ins.i32_const(0);
    ins.i32_ne();
    ins.br_if(1);
    ins.local_get(AMOUNT);
    ins.i32_eqz();
    ins.br_if(1);
    ins.br(0);
    ins.end();
    ins.end();
    // The future carries the listing error, if any.
    ins.local_get(FUTURE);
    ins.i32_const(p.buf as i32);
    ins.call(future_read);
    ins.local_set(CODE);
    ins.local_get(CODE);
    ins.i32_const(-1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(FUTURE);
    ins.call(wait_code);
    ins.local_set(CODE);
    ins.end();
    future_result(ins, p, CODE, OP_LIST, classify);
    ins.local_get(STREAM);
    ins.call(drop_stream);
    ins.local_get(FUTURE);
    ins.call(drop_future);
    ins.local_get(FD);
    ins.call(drop_);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.call(sort);
    ins.call(outcome);
}

pub(super) fn create_directory(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const ROOT: u32 = 1;
    const PTR: u32 = 2;
    const LEN: u32 = 3;
    const I: u32 = 4;
    let p = plan(b);
    let reset = b.rt("fs_reset");
    let root = b.rt("fs_root");
    let stat = b.rt("fs_stat");
    let outcome = b.rt("fs_outcome");
    let mkdir = b.rt("fs_mkdir");
    ins.call(reset);
    ins.call(root);
    ins.local_set(ROOT);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_set(PTR);
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(LEN);
    // A path that starts with `/` is absolute; the preopen cannot name it.
    ins.local_get(LEN);
    ins.if_(BlockType::Empty);
    ins.local_get(PTR);
    ins.i32_load8_u(mem(0, 1));
    ins.i32_const('/' as i32);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(p.err as i32);
    ins.i32_const(E_DENIED);
    ins.i32_store(mem(0, 4));
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.end();
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(LEN);
    ins.i32_ge_u();
    ins.if_(BlockType::Empty);
    // The whole path is the last prefix.
    ins.local_get(LEN);
    ins.i32_eqz();
    ins.br_if(2);
    ins.local_get(ROOT);
    ins.local_get(PTR);
    ins.local_get(LEN);
    ins.call(mkdir);
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.br(2);
    ins.end();
    ins.local_get(PTR);
    ins.local_get(I);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.i32_const('/' as i32);
    ins.i32_eq();
    ins.local_get(I);
    ins.i32_const(0);
    ins.i32_ne();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.local_get(ROOT);
    ins.local_get(PTR);
    ins.local_get(I);
    ins.call(mkdir);
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.end();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    // A file at the path (or in the middle of it) is NotADirectory.
    ins.local_get(ROOT);
    ins.local_get(0);
    ins.i32_const(1); // path-flags::symlink-follow
    ins.call(stat);
    ins.drop();
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.i32_const(p.ty as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(DT_DIRECTORY);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.i32_const(p.err as i32);
    ins.i32_const(E_NOT_DIR);
    ins.i32_store(mem(0, 4));
    ins.end();
    ins.call(outcome);
}

pub(super) fn remove(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const ROOT: u32 = 1;
    const PACKED: u32 = 2;
    const ST: u32 = 3;
    let p = plan(b);
    let reset = b.rt("fs_reset");
    let root = b.rt("fs_root");
    let stat = b.rt("fs_stat");
    let outcome = b.rt("fs_outcome");
    let await_ = b.rt("fs_await");
    let classify = b.rt("fs_classify");
    let unlink = import(
        b,
        "[async-lower][method]descriptor.unlink-file-at",
        vec![ValType::I32; 4],
        vec![ValType::I32],
    );
    let rmdir = import(
        b,
        "[async-lower][method]descriptor.remove-directory-at",
        vec![ValType::I32; 4],
        vec![ValType::I32],
    );
    ins.call(reset);
    ins.call(root);
    ins.local_set(ROOT);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    // The final symbolic link is removed itself, not its target: stat it without following,
    // so a link (to a directory, or dangling) is unlinked like a file.
    ins.local_get(ROOT);
    ins.local_get(0);
    ins.i32_const(0);
    ins.call(stat);
    ins.drop();
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.i32_const(p.ty as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(DT_DIRECTORY);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(ROOT);
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.i32_const(p.buf as i32);
    ins.call(rmdir);
    ins.local_set(PACKED);
    ins.else_();
    ins.local_get(ROOT);
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.i32_const(p.buf as i32);
    ins.call(unlink);
    ins.local_set(PACKED);
    ins.end();
    ins.local_get(PACKED);
    ins.call(await_);
    ins.local_set(ST);
    ins.local_get(ST);
    ins.i32_const(2);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.i32_const(p.err as i32);
    ins.i32_const(E_FAILURE);
    ins.i32_store(mem(0, 4));
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.i32_const(p.buf as i32);
    ins.i32_load8_u(mem(0, 1));
    ins.if_(BlockType::Empty);
    ins.i32_const(p.err as i32);
    ins.i32_const((p.buf + 4) as i32);
    ins.i32_const(OP_REMOVE);
    ins.call(classify);
    ins.i32_store(mem(0, 4));
    ins.end();
    ins.call(outcome);
}

/// `outcome_error(o)` / `outcome_message(o)` / `outcome_text(o)`: a fresh reference to the string
/// in `slot`.
pub(super) fn outcome_string(b: &mut Builder<'_>, ins: &mut InstructionSink, slot: u32) {
    const P: u32 = 1;
    let dup = b.rt("dup");
    ins.local_get(0);
    ins.i32_load(mem(slot, 4));
    ins.local_set(P);
    ins.local_get(P);
    ins.call(dup);
    ins.local_get(P);
}

pub(super) fn outcome_name_at(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const NAMES: u32 = 2;
    const P: u32 = 3;
    let dup = b.rt("dup");
    ins.local_get(0);
    ins.i32_load(mem(OUT_NAMES, 4));
    ins.local_set(NAMES);
    ins.local_get(NAMES);
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.local_get(1);
    ins.i32_const(3);
    ins.i32_shl();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(P);
    ins.local_get(P);
    ins.call(dup);
    ins.local_get(P);
}
