//! Reachability in the emitted component: an operation no reachable code performs is a stub
//! body, and a host interface only its body would use is not imported.
//!
//! This is the regression the design note reported: the WASM backend walked the Flow IR for the
//! imports and the Heap IR for body pruning, so an unused handler-operation body could be emitted
//! (and call a trap stub) while the import it needed was pruned. Both decisions now read
//! `emela_lir::analysis::reach`.

use crate::common;

use common::*;
use wasmparser::{Operator, Parser, Payload};

/// Whether any function body of the component's core module has an `i32.const value`.
fn any_body_has_const(bytes: &[u8], value: i32) -> bool {
    for payload in Parser::new(0).parse_all(bytes) {
        let Ok(Payload::CodeSectionEntry(body)) = payload else {
            continue;
        };
        let mut reader = body.get_operators_reader().expect("the operators parse");
        while !reader.eof() {
            if let Ok(Operator::I32Const { value: v }) = reader.read()
                && v == value
            {
                return true;
            }
        }
    }
    false
}

/// `Ping.pong` returns `1234567`, a value no other part of the probe program uses. If its body is
/// emitted, the constant is in the module; a stub is `unreachable` and has no constant.
#[test]
fn an_unperformed_operation_body_is_a_stub() {
    let bytes = compile_checked(
        r##"
import Std.Console.{Console, SystemConsole}

effect Ping {
  fn ping() -> Int
  fn pong() -> Int
}

handler Pinging: Ping {
  fn ping() { 1 }
  fn pong() { 1234567 }
}

fn main() {
  with SystemConsole {
    console = use Console
    console.print_line("start")
    with Pinging {
      n = (use Ping).ping()
      console.print_line("#{n}")
    }
  }
}
"##,
    );
    assert!(
        !any_body_has_const(&bytes, 1234567),
        "the body of the unperformed operation leaked into the module"
    );
    assert!(
        any_body_has_const(&bytes, 1),
        "the performed operation runs"
    );
}

/// A program that only reads the clock does not import the monotonic clock: the `sleep` operation
/// of `SystemClock` is built by the handler but never performed, so neither its body nor the host
/// interface it alone would use is emitted.
#[test]
fn a_host_only_an_unperformed_operation_uses_is_not_imported() {
    let bytes = compile_checked(
        r##"
import Std.Clock.{Clock, SystemClock}

fn main() {
  with SystemClock {
    dbg((use Clock).now())
  }
}
"##,
    );
    let imports = component_imports(&bytes);
    assert!(
        !imports.iter().any(|i| i.contains("monotonic")),
        "the unused sleep host leaked into the imports: {imports:?}"
    );
    assert!(
        imports.iter().any(|i| i.contains("clocks/system-clock")),
        "the used now host is imported: {imports:?}"
    );
}
