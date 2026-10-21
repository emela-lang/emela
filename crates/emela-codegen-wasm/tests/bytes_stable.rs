//! Pins the emitted bytes of two fixed programs.
//!
//! Splitting the emitter and the runtime into modules must not change the output. These programs
//! use only the features that are already supported (Int, String and closures; no Float), so a
//! change in the length or the hash of the module means the lowering changed.

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
