//! Pins the emitted bytes of fixed programs.
//!
//! A change in the length or the hash of a component means the lowering, the runtime or the
//! component wrapper changed. A refactoring must keep them. Every runtime function but the Float
//! ones is emitted whether a program calls it or not, so any program pins their bodies; the
//! programs here are chosen to also reach the emitter's lowerings: closures, strings, lists,
//! enums, options, the bundled `State`, Int and Int64 arithmetic, shifts, the Float remainder and
//! the test entry. The Float runtime is only registered for a program that uses a Float builtin,
//! so a change to it moves only `floats`, not the programs without one.
//!
//! The output is a WASI 0.3 component, so the hash covers the component (including its metadata),
//! not just the core module.
//!
//! The bytes depend on everything before the emitter too (the Prelude, the Core IR passes), so a
//! change there can also fail this test. When the output changes on purpose, check that the
//! change comes from what you changed, then paste the values from the failure message, which
//! lists every program whose output changed, in place of the old ones.

use crate::common;

use common::*;
use emela_codegen_wasm::WasmOptions;

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
  |b: Int| a + b
}
fn main() {
  add2 = adder(2)
  dbg(add2(40))
  dbg("x=#{1 + 2}!")
  dbg(String.append("ab", "cd"))
  dbg("ab" == "ab")
}
"#,
        12886,
        0xce3b_f855_b1c2_b1d3,
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
  |b: Int| a + b
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
        23978,
        0xc3b2_ca3d_6a48_2881,
    ),
    (
        "floats",
        r#"
fn main() {
  dbg(1.5)
  dbg(0.1 + 0.2)
  dbg(1.0e21)
  dbg(0.0 / 0.0)
  dbg(Float.floor(-1.5))
  dbg(Float.to_string(Float.parse("2.5") escape { ParseError(_) -> 0.0 }))
}
"#,
        22404,
        0x686a_4035_cf6c_d594,
    ),
    (
        "platform_console",
        r#"
import Std.Console.{Console}
import Std.Platform.{Platform}

fn main() {
  with Platform {
    (use Console).print_line("platform")
  }
}
"#,
        36147,
        0x949d_1fef_ac09_6fd4,
    ),
    (
        "state_and_options",
        r#"
import Std.State.{State}

fn main() {
  (_, final) = State.run([], || {
    st = use State[List[String]]
    st.update(|xs| List.append(xs, ["x"]))
    st.update(|xs| List.append(xs, ["y"]))
  })
  dbg(final)
  dbg(Int.parse("42"))
  dbg(Int.checked_add(1, 2))
  dbg(String.from_code_point(65))
}
"#,
        15596,
        0xf0ec_328c_81ee_7384,
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
        13252,
        0xde55_f3e1_8caf_92c1,
    ),
];

/// `(name, source, test functions, expected length, expected FNV-1a)`, compiled with
/// `Entry::Tests`.
const TEST_PROGRAMS: &[(&str, &str, &[&str], usize, u64)] = &[(
    "tests",
    r#"
error Nope(why: String) derive Show
fn half(n: Int) -> Int fails Nope {
  if n % 2 != 0 { fail Nope(why: "odd \"quoted\"") }
  n / 2
}
pub fn passes() -> Int fails Nope { half(4) }
pub fn fails_with_an_error() -> Int fails Nope { half(3) }
pub fn raises_a_defect() { panic("boom") }
"#,
    &["passes", "fails_with_an_error", "raises_a_defect"],
    13181,
    0xf3ae_1892_a5c9_be47,
)];

/// Compares each `(name, bytes, expected length, expected FNV-1a)` and fails once, listing the new
/// values of every program whose output changed in the form the tables use.
fn check(results: impl IntoIterator<Item = (&'static str, Vec<u8>, usize, u64)>) {
    let changed: Vec<String> = results
        .into_iter()
        .filter_map(|(name, bytes, expected_len, expected_hash)| {
            let (len, hash) = (bytes.len(), fnv1a(&bytes));
            (len != expected_len || hash != expected_hash).then(|| {
                let hex = format!("{hash:016x}");
                let groups: Vec<&str> = (0..16).step_by(4).map(|i| &hex[i..i + 4]).collect();
                format!("{name}: {len}, 0x{},", groups.join("_"))
            })
        })
        .collect();
    assert!(
        changed.is_empty(),
        "the emitted bytes changed (the components are still valid); the new values are:\n{}",
        changed.join("\n")
    );
}

#[test]
fn emitted_bytes_do_not_change() {
    check(
        PROGRAMS
            .iter()
            .map(|&(name, src, len, hash)| (name, compile_checked(src), len, hash)),
    );
}

#[test]
fn emitted_test_bytes_do_not_change() {
    check(TEST_PROGRAMS.iter().map(|&(name, src, tests, len, hash)| {
        (name, compile_tests_checked(src, tests), len, hash)
    }));
}

/// The opt-in E2 form: an array of an all-scalar record stores the element inline. This is a
/// feature pin for `WasmOptions::aggregate_arrays`, not a change to a boxed pin above; the option
/// is off by default and every other pin stays as it was.
#[test]
fn emitted_inline_array_bytes_do_not_change() {
    const SRC: &str = r#"
import Std.Array.{Array}

type Point(x: Int, y: Int)

fn main() {
  a = Array.from_list([Point(x: 1, y: 2), Point(x: 3, y: 4)])
  dbg(a)
  dbg(Array.set(a, 0, Point(x: 5, y: 6)))
  dbg(Array.get(a, 1))
  dbg(Array.to_list(a))
}
"#;
    let output = to_output_with(
        SRC,
        WasmOptions {
            aggregate_arrays: true,
            ..WasmOptions::default()
        },
    );
    wasmparser::validate(&output.module).expect("the inline-array module is valid");
    // The C2 profitability rule changes the borrows of the call sites in this program.
    check([("inline_array", output.module, 15647, 0x5e37_5b5c_c04d_e736)]);
}
