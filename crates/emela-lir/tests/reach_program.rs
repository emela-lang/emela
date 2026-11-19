//! The design note's two-operation probe on a real program, at both LIR stages.
//!
//! A handler with `ping` and `pong` where only `ping` is performed, and `SystemConsole` where
//! only `print_line` is used. The unused operations must be `referenced` but not `called`, and
//! the hosts they alone call must not be reachable. This is the discrepancy the WASM backend had
//! between the Flow-IR walk used for imports and the Heap-IR walk used for body pruning.

#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

use emela_core::Roots;
use emela_lir::Module;
use emela_lir::analysis::reach::Reachability;

const PROGRAM: &str = r##"
import Std.Console.{Console, SystemConsole}

effect Ping {
  fn ping() -> Int
  fn pong() -> Int
}

handler Pinging: Ping {
  fn ping() { 1 }
  fn pong() { 2 }
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
"##;

fn lowered() -> Module {
    let core = support::try_lower(&[("main.emel", PROGRAM)], Roots::Exported)
        .unwrap_or_else(|e| panic!("lowering failed: {e:?}"));
    emela_lir::from_core(core).unwrap_or_else(|e| panic!("from_core failed: {e:?}"))
}

fn named(m: &Module, name: &str) -> emela_lir::FnId {
    m.functions
        .iter()
        .find(|(_, f)| f.name == name)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no function `{name}`"))
}

#[test]
fn only_the_performed_operation_runs_on_the_flow_ir() {
    let m = lowered();
    let main = named(&m, "main");
    let reach = Reachability::walk(&m, &[main]);
    let console = m
        .core
        .effects
        .iter()
        .find(|(_, e)| e.name == "Std.Console.Console")
        .map(|(id, _)| id)
        .expect("the Console effect");
    let ping = m
        .core
        .effects
        .iter()
        .find(|(_, e)| e.name == "Ping")
        .map(|(id, _)| id)
        .expect("the Ping effect");
    assert!(reach.performed().contains(&(console, 0)));
    assert!(!reach.performed().contains(&(console, 1)));
    assert!(reach.performed().contains(&(ping, 0)));
    assert!(!reach.performed().contains(&(ping, 1)));
}

#[test]
fn the_unused_operations_are_referenced_but_not_called_on_the_heap_ir() {
    let mut m = lowered();
    emela_lir::to_heap(&mut m);
    let main = named(&m, "main");
    let reach = Reachability::walk(&m, &[main]);

    // Only `print_line` is used, so only the first closure of each handler runs. The other is
    // built (the handler record names it) but never performed.
    for (called, referenced) in [
        ("Pinging$closure0", "Pinging$closure1"),
        (
            "Std.Console.SystemConsole$closure0",
            "Std.Console.SystemConsole$closure1",
        ),
    ] {
        let called = named(&m, called);
        let referenced = named(&m, referenced);
        assert!(
            reach.is_called(called),
            "`{}` runs",
            m.functions[called].name
        );
        assert!(reach.is_referenced(referenced));
        assert!(
            !reach.is_called(referenced),
            "`{}` is built but never performed",
            m.functions[referenced].name
        );
    }

    // The host called by the unused operation is not reachable, while the one its sibling calls
    // is. Imports and bodies therefore read the same set.
    let log = named(&m, "Std.Console.console_log");
    let error = named(&m, "Std.Console.console_error");
    assert!(reach.externals().contains(&log));
    assert!(!reach.externals().contains(&error));
}
