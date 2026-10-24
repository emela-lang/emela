//! Failures and the test result protocol (spec 13.4).

use super::*;

pub(super) const FUNCS: &[Sig] = &[
    ("show_failure", &[ValType::I32], &[ValType::I32]),
    ("json_string", &[ValType::I32], &[]),
    ("test_result", &[ValType::I32], &[]),
    ("test_done", &[], &[]),
    ("finish", &[], &[]),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "show_failure" => i32s(3), // desc, show, result
        "json_string" => i32s(3),  // i, byte, hex
        "test_result" => i32s(3),  // kind, message, error
        "finish" => i32s(1),       // shown
        _ => Vec::new(),
    }
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "show_failure" => show_failure(b, ins),
        "json_string" => json_string(b, ins),
        "test_result" => test_result(b, ins),
        "test_done" => test_done(b, ins),
        "finish" => finish(b, ins),
        _ => return false,
    }
    true
}

// ---- failures and the test protocol (spec 13.4) ----

/// The litteral pieces of the test result line (the JS test runner's protocol).
const TEST_PREFIX: &[u8] = b"\x1eemela-test {\"event\":\"result\",\"name\":";
const TEST_OK: &[u8] = b",\"outcome\":\"ok\"}\n";
const TEST_DEFECT: &[u8] = b",\"outcome\":\"defect\"";
const TEST_ERROR: &[u8] = b",\"outcome\":\"error\"";
const TEST_MESSAGE: &[u8] = b",\"message\":";
const TEST_TAIL: &[u8] = b"}\n";
const TEST_DONE_LINE: &[u8] = b"\x1eemela-test {\"event\":\"done\"}\n";

/// `show_failure(e)`: the message of an error value, calling the `show` function of its descriptor
/// (spec 10.6). Returns a null reference when there is none (or a defect happened while showing).
fn show_failure(b: &mut Builder, ins: &mut InstructionSink) {
    const D: u32 = 1;
    const SHOW: u32 = 2;
    const RESULT: u32 = 3;
    ins.local_get(0);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(8, 4));
    ins.local_set(D);
    ins.local_get(D);
    ins.i32_load(mem(DESC_SHOW, 4));
    ins.local_tee(SHOW);
    ins.i32_const(-1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.call(rt(b, "drop"));
    ins.i32_const(0);
    ins.return_();
    ins.end();
    let ty = b.call_ty(&[Repr::Ref], Repr::Ref);
    ins.local_get(0);
    ins.local_get(SHOW);
    ins.call_indirect(0, ty);
    ins.local_set(RESULT);
    ins.global_get(G_DEFECT);
    ins.if_(BlockType::Empty);
    ins.local_get(RESULT);
    ins.call(rt(b, "drop"));
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(RESULT);
}

/// Writes `bytes` (a static string object) to `fd`.
fn write_static(b: &mut Builder, ins: &mut InstructionSink, fd: i32, bytes: &[u8]) {
    let s = b.string_literal(bytes);
    ins.i32_const(fd);
    ins.i32_const((s + HDR + 4) as i32);
    ins.i32_const(bytes.len() as i32);
    ins.call(rt(b, "write"));
}

/// Writes `s` to stdout as a JSON string (spec 13.4): `"` at both ends and the escapes the JS test
/// runner uses (`\"`, `\\`, `\n`, and `\u00XX` for the other control characters).
fn json_string(b: &mut Builder, ins: &mut InstructionSink) {
    const I: u32 = 1;
    const B: u32 = 2;
    const HEX: u32 = 3;
    let write = rt(b, "write");
    // Opening quote.
    ins.i32_const(JSONBUF as i32);
    ins.i32_const('"' as i32);
    ins.i32_store8(mem(0, 1));
    ins.i32_const(1);
    ins.i32_const(JSONBUF as i32);
    ins.i32_const(1);
    ins.call(write);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(I);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.local_set(B);
    // `"` or `\`.
    ins.local_get(B);
    ins.i32_const('"' as i32);
    ins.i32_eq();
    ins.local_get(B);
    ins.i32_const('\\' as i32);
    ins.i32_eq();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    write_escape_byte(ins, write, B);
    ins.else_();
    // `\n`.
    ins.local_get(B);
    ins.i32_const('\n' as i32);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    write_two_bytes(ins, write, b'\\', b'n');
    ins.else_();
    // Other control characters: `\u00XX`.
    ins.local_get(B);
    ins.i32_const(0x20);
    ins.i32_lt_u();
    ins.local_get(B);
    ins.i32_const(0x7F);
    ins.i32_eq();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.i32_const(JSONBUF as i32);
    ins.i32_const('\\' as i32);
    ins.i32_store8(mem(0, 1));
    ins.i32_const((JSONBUF + 1) as i32);
    ins.i32_const('u' as i32);
    ins.i32_store8(mem(0, 1));
    ins.i32_const((JSONBUF + 2) as i32);
    ins.i32_const('0' as i32);
    ins.i32_store8(mem(0, 1));
    ins.i32_const((JSONBUF + 3) as i32);
    ins.i32_const('0' as i32);
    ins.i32_store8(mem(0, 1));
    hex_digit(ins, JSONBUF + 4, B, 4, HEX);
    hex_digit(ins, JSONBUF + 5, B, 15, HEX);
    ins.i32_const(1);
    ins.i32_const(JSONBUF as i32);
    ins.i32_const(6);
    ins.call(write);
    ins.else_();
    ins.i32_const(JSONBUF as i32);
    ins.local_get(B);
    ins.i32_store8(mem(0, 1));
    ins.i32_const(1);
    ins.i32_const(JSONBUF as i32);
    ins.i32_const(1);
    ins.call(write);
    ins.end();
    ins.end();
    ins.end();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    // Closing quote.
    ins.i32_const(JSONBUF as i32);
    ins.i32_const('"' as i32);
    ins.i32_store8(mem(0, 1));
    ins.i32_const(1);
    ins.i32_const(JSONBUF as i32);
    ins.i32_const(1);
    ins.call(write);
}

/// Writes `\` and the byte in local `of` to stdout.
fn write_escape_byte(ins: &mut InstructionSink, write: u32, of: u32) {
    ins.i32_const(JSONBUF as i32);
    ins.i32_const('\\' as i32);
    ins.i32_store8(mem(0, 1));
    ins.i32_const((JSONBUF + 1) as i32);
    ins.local_get(of);
    ins.i32_store8(mem(0, 1));
    ins.i32_const(1);
    ins.i32_const(JSONBUF as i32);
    ins.i32_const(2);
    ins.call(write);
}

/// Writes the two bytes `a` and `b` to stdout.
fn write_two_bytes(ins: &mut InstructionSink, write: u32, a: u8, b: u8) {
    ins.i32_const(JSONBUF as i32);
    ins.i32_const(a as i32);
    ins.i32_store8(mem(0, 1));
    ins.i32_const((JSONBUF + 1) as i32);
    ins.i32_const(b as i32);
    ins.i32_store8(mem(0, 1));
    ins.i32_const(1);
    ins.i32_const(JSONBUF as i32);
    ins.i32_const(2);
    ins.call(write);
}

/// Writes one lowercase hex digit of the nibble `n` (local) at `addr`.
fn hex_digit(ins: &mut InstructionSink, addr: u32, n: u32, mask: u32, hex: u32) {
    ins.i32_const(addr as i32);
    ins.local_get(n);
    if mask == 4 {
        ins.i32_const(4);
        ins.i32_shr_u();
    } else {
        ins.i32_const(15);
        ins.i32_and();
    }
    ins.local_tee(hex);
    ins.i32_const(10);
    ins.i32_lt_u();
    ins.if_(BlockType::Result(ValType::I32));
    ins.local_get(hex);
    ins.i32_const('0' as i32);
    ins.i32_add();
    ins.else_();
    ins.local_get(hex);
    ins.i32_const('a' as i32 - 10);
    ins.i32_add();
    ins.end();
    ins.i32_store8(mem(0, 1));
}

/// Reports one test's result (spec 13.4): the name, the outcome, and the failure's message.
fn test_result(b: &mut Builder, ins: &mut InstructionSink) {
    const KIND: u32 = 1;
    const MESSAGE: u32 = 2;
    const E: u32 = 3;
    write_static(b, ins, 1, TEST_PREFIX);
    ins.local_get(0);
    ins.call(rt(b, "json_string"));
    ins.i32_const(0);
    ins.local_set(KIND);
    ins.i32_const(0);
    ins.local_set(MESSAGE);
    ins.global_get(G_DEFECT);
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(KIND);
    ins.global_get(G_DEFECT);
    ins.local_set(MESSAGE);
    ins.i32_const(0);
    ins.global_set(G_DEFECT);
    ins.else_();
    ins.global_get(G_FAILURE);
    ins.if_(BlockType::Empty);
    ins.global_get(G_FAILURE);
    ins.local_set(E);
    ins.i32_const(0);
    ins.global_set(G_FAILURE);
    ins.i32_const(2);
    ins.local_set(KIND);
    ins.local_get(E);
    ins.call(rt(b, "show_failure"));
    ins.local_set(MESSAGE);
    ins.end();
    ins.end();
    ins.local_get(KIND);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    write_static(b, ins, 1, TEST_OK);
    ins.else_();
    ins.local_get(KIND);
    ins.i32_const(1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    write_static(b, ins, 1, TEST_DEFECT);
    ins.else_();
    write_static(b, ins, 1, TEST_ERROR);
    ins.end();
    ins.local_get(MESSAGE);
    ins.i32_eqz();
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    write_static(b, ins, 1, TEST_MESSAGE);
    ins.local_get(MESSAGE);
    ins.call(rt(b, "json_string"));
    ins.local_get(MESSAGE);
    ins.call(rt(b, "drop"));
    ins.end();
    write_static(b, ins, 1, TEST_TAIL);
    ins.end();
}

/// Writes the closing `done` line.
fn test_done(b: &mut Builder, ins: &mut InstructionSink) {
    write_static(b, ins, 1, TEST_DONE_LINE);
}

/// Reports a pending defect (exit 101) or failure (exit 1) at the end of the program, like the C
/// runtime's `em_exit` (spec 14.3).
fn finish(b: &mut Builder, ins: &mut InstructionSink) {
    const SHOWN: u32 = 0;
    ins.global_get(G_DEFECT);
    ins.if_(BlockType::Empty);
    write_static(b, ins, 2, b"defect: ");
    ins.i32_const(2);
    ins.global_get(G_DEFECT);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.global_get(G_DEFECT);
    ins.i32_load(mem(HDR, 4));
    ins.call(rt(b, "write"));
    write_static(b, ins, 2, b"\n");
    ins.i32_const(101);
    ins.call(rt(b, "exit"));
    ins.end();
    ins.global_get(G_FAILURE);
    ins.if_(BlockType::Empty);
    ins.global_get(G_FAILURE);
    ins.local_set(SHOWN);
    ins.i32_const(0);
    ins.global_set(G_FAILURE);
    write_static(b, ins, 2, b"error: ");
    ins.local_get(SHOWN);
    ins.call(rt(b, "show_failure"));
    ins.local_set(SHOWN);
    ins.local_get(SHOWN);
    ins.i32_eqz();
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(2);
    ins.local_get(SHOWN);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(SHOWN);
    ins.i32_load(mem(HDR, 4));
    ins.call(rt(b, "write"));
    ins.end();
    write_static(b, ins, 2, b"\n");
    ins.i32_const(1);
    ins.call(rt(b, "exit"));
    ins.end();
}
