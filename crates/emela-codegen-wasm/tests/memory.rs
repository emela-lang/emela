//! Freed objects go back to the allocator: a program that keeps making and dropping the same
//! objects runs in a fixed amount of linear memory. Each program here runs under a memory limit
//! far below what it allocates in total. Skipped without `wasmtime`.

mod common;

use common::*;

/// The linear memory each program may grow to: six 64 KiB pages.
const LIMIT: u32 = 6 * 65536;

/// Runs `src` with linear memory limited to [`LIMIT`], expecting it to finish with `stderr`.
fn run_limited(name: &str, src: &str, stderr: &str) {
    let Some(_) = wasmtime() else { return };
    let bytes = compile_checked(src);
    let run = run_bytes_with_command(&out_dir(name), &bytes, |command| {
        command.arg("-W").arg(format!("max-memory-size={LIMIT}"));
    });
    assert_eq!(run.code, 0, "{name}: {}", run.stderr);
    assert_eq!(run.stderr, stderr, "{name}");
}

#[test]
fn dropping_a_closure_frees_what_it_captured() {
    // Each round drops a closure whose environment holds a list of 100 cells: the cells are freed
    // after the environment, not only the environment itself.
    run_limited(
        "memory-closure",
        r#"
fn make(n: Int) -> fn() -> Int {
  xs = List.map(List.range(0, n), fn(i) { i * 2 })
  fn() { List.length(xs) }
}

fn rounds(n: Int, total: Int) -> Int {
  if n == 0 {
    total
  } else {
    f = make(100)
    rounds(n - 1, total + f())
  }
}

fn main() {
  dbg(rounds(2000, 0))
}
"#,
        "200000\n",
    );
}
