//! Pins the emitted bytes of fixed programs.
//!
//! Splitting the emitter and the runtime into modules must not change the output. Every runtime
//! function is emitted whether a program calls it or not, so any program pins their bodies; the
//! programs here are chosen to also reach the emitter's lowerings: closures, strings, lists, enums,
//! options, wasm externals, Int and Int64 arithmetic, shifts and the Float remainder. A change in
//! the length or the hash of the module means the lowering changed.

mod common;

use common::*;

/// FNV-1a, 64 bits: enough to catch an accidental change without another dependency.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// `(name, source, expected length, expected FNV-1a)`.
const PROGRAMS: &[(&str, &str, usize, u64)] = &[
    (
        "closures_and_strings",
        r#"
fn adder(a: Int) -> fn(Int) -> Int {
  fn(b: Int) { a + b }
}
fn main() {
  add2 = adder(2)
  dbg(add2(40))
  dbg("x=#{1 + 2}!")
  dbg(String.append("ab", "cd"))
  dbg("ab" == "ab")
}
"#,
        6888,
        0xa877_d7bb_4404_91f4,
    ),
    (
        "lists_enums_strings",
        r#"
enum Maybe {
  Missing
  Found(value: Int)
}
fn get(o: Maybe) -> Int {
  o match {
    Missing -> 0
    Found(value:) -> value
  }
}
fn total(xs: List[Int], acc: Int) -> Int {
  xs match {
    [] -> acc
    [x, ..rest] -> total(rest, acc + x)
  }
}
fn adder(a: Int) -> fn(Int) -> Int {
  fn(b: Int) { a + b }
}
fn main() {
  add = adder(1)
  dbg(total([1, 2, 3, 4], 0))
  dbg(get(Found(value: 42)))
  dbg(get(Missing))
  dbg(add(41))
  dbg(String.join(String.split("a,b,c", ","), "-"))
  dbg(String.trim("  hi  "))
}
"#,
        7304,
        0xe6db_0e6a_d5a9_5794,
    ),
    (
        "externals_and_options",
        r#"
effect Clock {
  fn now() -> Int
}

@external(wasm, "env", "now")
fn host_now() -> Int

handler WasmClock: Clock {
  fn now() { host_now() }
}

fn main() {
  with WasmClock { dbg((use Clock).now()) }
  dbg(Int.parse("42"))
  dbg(Int.checked_add(1, 2))
  dbg(String.from_code_point(65))
}
"#,
        7664,
        0xae57_5f46_4529_1fdc,
    ),
    (
        "numeric",
        r#"
fn main() {
  a = 0 - 17
  dbg(a / 5)
  dbg(a % 5)
  dbg(-a)
  dbg(Int.shift_left(1, 33))
  dbg(Int.shift_right(a, 2))
  dbg(Int.shift_right_unsigned(a, 28))
  dbg(Int.bit_xor(Int.bit_and(12, 10), Int.bit_not(0)))
  big = Int64.from_int(2147483647) * Int64.from_int(3)
  seven = Int64.from_int(7)
  dbg(Int64.to_string(big / seven))
  dbg(Int64.to_string(big % seven))
  dbg(Int64.to_string(-big))
  dbg(Int64.to_string(Int64.shift_left(big, 2)))
  dbg(Int64.to_string(Int64.shift_right(-big, 3)))
  dbg(Int64.to_string(Int64.shift_right_unsigned(-big, 60)))
  dbg(Int64.to_int(big))
  dbg(Float.from_int(7) % Float.from_int(2) > 0.5)
}
"#,
        7175,
        0x5dc0_2e86_5969_fdc3,
    ),
];

#[test]
fn emitted_bytes_do_not_change() {
    for &(name, src, expected_len, expected_hash) in PROGRAMS {
        let bytes = compile_checked(src);
        assert_eq!(bytes.len(), expected_len, "{name}: module length changed");
        assert_eq!(
            fnv1a(&bytes),
            expected_hash,
            "{name}: emitted bytes changed (the module is still valid)"
        );
    }
}
