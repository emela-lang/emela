//! The `Std.Random` internal hosts (`emela:random` `int` / `float`), which fill a buffer with WASI
//! `random_get`. The values are nondeterministic, so the test checks the shape: `int` prints as a
//! 32-bit integer and `float` lies in `[0, 1)`.

mod common;

use common::*;

#[test]
fn system_random_int_and_float() {
    let Some(run) = run(
        "random",
        r#"
import Std.Console.{Console, SystemConsole}
import Std.Random.{Random, SystemRandom}

fn in_range(f: Float) -> Bool {
  (f >= 0.0) && (f < 1.0)
}

fn main() {
  with SystemConsole, SystemRandom {
    random = use Random
    console = use Console
    n = random.int()
    console.print_line("int #{n}")
    console.print_line("float in range #{in_range(random.float())}")
    console.print_line("float in range #{in_range(random.float())}")
  }
}
"#,
    ) else {
        return;
    };
    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert_eq!(run.stderr, "");
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{:?}", run.stdout);
    let int = lines[0]
        .strip_prefix("int ")
        .and_then(|n| n.parse::<i32>().ok());
    assert!(
        int.is_some(),
        "`int` is not a 32-bit integer: {:?}",
        lines[0]
    );
    assert_eq!(lines[1], "float in range True");
    assert_eq!(lines[2], "float in range True");
}
