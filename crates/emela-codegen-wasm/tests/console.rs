//! The `Std.Console` internal hosts (`emela:console` `log` / `error`). `SystemConsole.print_line`
//! writes its line and a newline to stdout (fd 1) and `print_error_line` to stderr (fd 2), like
//! `print_string` and `debug_print` with the `String` crossing as a linear-memory pointer.

mod common;

use common::*;

#[test]
fn system_console_writes_stdout_and_stderr() {
    let Some(run) = run(
        "console",
        r#"
import Std.Console.{Console, SystemConsole}

fn main() {
  with SystemConsole {
    console = use Console
    console.print_line("hi")
    console.print_error_line("oops")
    console.print_line("bye")
  }
}
"#,
    ) else {
        return;
    };
    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert_eq!(run.stdout, "hi\nbye\n");
    assert_eq!(run.stderr, "oops\n");
}
