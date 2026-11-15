//! The allocator of the C runtime: size-class bins for fixed-size objects, the static empty
//! evidence table, and the allocation statistics that a `-DEMELA_ALLOC_STATS` build prints.
//!
//! The bins are checked from C, because what they guarantee (a block of one size class coming
//! back for the next object of that class, and not for one of another class) is not visible from
//! Emela. The statistics are checked through a compiled Emela program, because the contract with
//! the benchmark harness is the text on standard error.

mod common;

use std::process::{Command, Output};
use std::time::Duration;

use common::{PRELUDE, build_and_run, cc, out_dir, run_with_timeout, to_c};
use emela_codegen_c::{
    EMELA_H, EMELA_H_FILE, RUNTIME_C, RUNTIME_C_FILE, RUNTIME_H, RUNTIME_H_FILE,
};

/// Checks the bins and the empty evidence table. It allocates and frees through the runtime
/// itself, then ends with `em_exit`, whose leak check (the runtime is compiled with
/// `-DEMELA_LEAK_CHECK`) must find nothing live.
const RUNTIME_TEST: &str = r#"
#include <stdio.h>

#include "emela_runtime.h"

static int failures = 0;

static void check(int ok, const char *what) {
  if (!ok) {
    fprintf(stderr, "check failed: %s\n", what);
    failures++;
  }
}

/* Wider than a bin (16 + 40 * 8 = 336 bytes). */
static const uint8_t big_refs[40] = {0};
static const em_desc big_desc = {EM_KIND_FIELDS, 40, 0, -1, "big", NULL, big_refs};

/* A neighbour of the cons cell's class that malloc may serve from the same bucket. */
static const uint8_t trio_refs[3] = {0};
static const em_desc trio_desc = {EM_KIND_FIELDS, 3, 0, -1, "trio", NULL, trio_refs};

int main(void) {
  /* The empty evidence table is one static object: never counted, never freed. */
  em_ref e1 = em_ev_new(NULL, 0);
  em_ref e2 = em_ev_new(NULL, 0);
  check(e1 == e2, "an empty evidence table is the same static object");
  check(e1->hdr.u.h.rc == 0, "the empty evidence table is static data");
  check(e1->slots[0].l == 0, "the empty evidence table has no bindings");
  em_drop(e1);
  check(e1->hdr.u.h.rc == 0, "dropping static data leaves it alone");

  /* Extending it copies nothing, and the binding is found. */
  em_ref ev = em_ev_new(e1, 1);
  em_ev_set(ev, 1, 0, 7, NULL);
  check(ev != e1, "extending an empty table allocates");
  check(ev->slots[0].l == 1, "the table has one binding");
  check(em_ev_lookup(ev, 7) == NULL, "the binding is found");
  em_drop(ev);
  em_drop(e2);

  /* A fixed-size object comes back from the bin of its class, most recently freed first. */
  em_ref a = em_alloc(&em_desc_cons_val, 2);
  em_ref b = em_alloc(&em_desc_cons_val, 2);
  check(a != b, "two live blocks differ");
  a->slots[0].i = 1;
  a->slots[1].p = NULL;
  b->slots[0].i = 2;
  b->slots[1].p = NULL;
  em_free(a);
  em_free(b);
  em_ref c = em_alloc(&em_desc_cons_val, 2);
  check(c == b, "the most recently freed block comes back first");
  em_free(c);
  c = em_alloc(&em_desc_cons_val, 2);
  check(c == b, "and the one freed before it next");
  em_free(c);

  /* A request of another class does not take a block of this one: the classes are slots, not
   * whatever buckets malloc uses. */
  em_ref two = em_alloc(&em_desc_cons_val, 2);
  em_free(two);
  em_ref three = em_alloc(&trio_desc, 3);
  check(three != two, "a three-slot object does not take a two-slot block");
  three->slots[2].i = 7;
  em_free(three);
  em_ref one = em_alloc(&em_desc_box_val, 1);
  check(one != two, "a one-slot object does not take a two-slot block");
  one->slots[0].i = 3;
  em_ref again = em_alloc(&em_desc_cons_val, 2);
  check(again == two, "the two-slot block is still in its own bin");
  em_free(again);
  em_free(one);

  /* Objects over 256 bytes take malloc and are freed (a wrong free shows under ASan). */
  em_ref big = em_alloc(&big_desc, 40);
  for (int i = 0; i < 40; i++) big->slots[i].i = i;
  check(big->slots[39].i == 39, "a block over 256 bytes is writable");
  em_free(big);

  /* A whole list is released through the bins, without recursion. */
  em_ref list = NULL;
  for (int i = 0; i < 1000; i++) {
    em_ref cell = em_alloc(&em_desc_cons_val, 2);
    cell->slots[0].i = i;
    cell->slots[1].p = list;
    list = cell;
  }
  em_drop(list);

  if (failures != 0) return 1;
  /* The leak check runs here: 99 if any allocation is still live. */
  return em_exit(NULL);
}
"#;

/// Builds a C program against the runtime and runs it, with the leak check compiled in. The
/// program need not reach `em_exit`, so that a test can check how it failed.
fn run_c_output(name: &str, main_c: &str) -> Output {
    let cc = cc().expect("checked by the caller");
    let dir = out_dir(name);
    std::fs::write(dir.join("main.c"), main_c).unwrap();
    std::fs::write(dir.join(RUNTIME_C_FILE), RUNTIME_C).unwrap();
    std::fs::write(dir.join(RUNTIME_H_FILE), RUNTIME_H).unwrap();
    std::fs::write(dir.join(EMELA_H_FILE), EMELA_H).unwrap();
    let built = Command::new(cc)
        .args([
            "-std=c11",
            "-fno-strict-aliasing",
            "-pthread",
            "-I.",
            "-O1",
            "-DEMELA_LEAK_CHECK",
            "-o",
            "main",
            "main.c",
            RUNTIME_C_FILE,
            "-lm",
        ])
        .current_dir(&dir)
        .output()
        .expect("cannot start cc");
    assert!(
        built.status.success(),
        "cc failed:\n{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let mut command = Command::new(dir.join("main"));
    command.env_remove("EMELA_LEAK_CHECK").current_dir(&dir);
    run_with_timeout(command, Duration::from_secs(120)).expect("the program timed out")
}

/// Builds a C program against the runtime and runs it. The program ends with `em_exit`, so the
/// leak check is part of what it checks.
fn run_c(name: &str, main_c: &str) -> (i32, String, String) {
    let ran = run_c_output(name, main_c);
    (
        ran.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&ran.stdout).into_owned(),
        String::from_utf8_lossy(&ran.stderr).into_owned(),
    )
}

#[test]
fn fixed_size_objects_are_reused_by_size_class() {
    if cc().is_none() {
        return;
    }
    let (code, stdout, stderr) = run_c("alloc-bins", RUNTIME_TEST);
    assert_eq!((code, stdout.as_str(), stderr.as_str()), (0, "", ""));
}

/// The lines of `emela-alloc NAME COUNT` and the last `emela-alloc-total COUNT`, checked against
/// the contract with the benchmark harness: most numerous first, ties by name, and the total is
/// the sum of the counts.
fn check_stats(stderr: &str) -> Vec<(String, u64)> {
    let mut lines: Vec<&str> = stderr.lines().collect();
    let total: u64 = lines
        .pop()
        .expect("a total line")
        .strip_prefix("emela-alloc-total ")
        .expect("the total last")
        .parse()
        .expect("a total");
    let mut counts = Vec::new();
    for line in lines {
        let rest = line
            .strip_prefix("emela-alloc ")
            .unwrap_or_else(|| panic!("a name and a count, not {line:?}"));
        let (name, count) = rest.rsplit_once(' ').expect("a name and a count");
        counts.push((name.to_owned(), count.parse::<u64>().expect("a count")));
    }
    let sum: u64 = counts.iter().map(|(_, n)| n).sum();
    assert_eq!(total, sum, "the total is the sum of the counts");
    for pair in counts.windows(2) {
        assert!(
            pair[0].1 > pair[1].1 || (pair[0].1 == pair[1].1 && pair[0].0 < pair[1].0),
            "the report is by count, and ties by name: {pair:?}"
        );
    }
    counts
}

#[test]
fn ascii_case_conversion_allocates_only_its_result() {
    // `String.to_ascii_lowercase` and `to_ascii_uppercase` copy the bytes once: one String and
    // nothing else, not the list of code points, the list of one-character strings and the Some
    // of `from_code_point` that a conversion written with those builtins allocates.
    if cc().is_none() {
        return;
    }
    let src = format!(
        r#"{PRELUDE}
fn main() {{
  print(String.to_ascii_lowercase("aAzZ éÄß 日本語 🙂aZ🙂"))
  print(String.to_ascii_uppercase("aAzZ éÄß 日本語 🙂aZ🙂"))
}}
"#
    );
    let out = to_c(&src).expect("the program compiles");
    let got = build_and_run(
        &out_dir("ascii-case-alloc"),
        &out,
        &["-O1", "-DEMELA_ALLOC_STATS"],
    );
    assert_eq!(got.code, 0, "{}", got.stderr);
    let report: Vec<&str> = got
        .stderr
        .lines()
        .skip_while(|line| !line.starts_with("emela-alloc "))
        .collect();
    assert!(!report.is_empty(), "a report");
    let counts = check_stats(&report.join("\n"));
    let count = |n: &str| counts.iter().find(|(name, _)| name == n).map(|(_, c)| *c);
    assert_eq!(count("String"), Some(2), "one String per conversion");
    assert_eq!(count("cons"), None, "no list is built");
    assert_eq!(count("Some"), None, "no Option is built");
}

#[test]
fn alloc_stats_are_reported_in_a_measurement_build() {
    if cc().is_none() {
        return;
    }
    let src = format!(
        r#"{PRELUDE}
effect Name {{
  fn name() -> String
}}

handler Res: Name {{
  init {{ Res }}
  fn name() {{ "res" }}
}}

fn main() {{
  xs = [1, 2, 3] |> List.map(|x| x + 1)
  dbg(Int.to_string(List.fold(xs, 0, |acc, x| acc + x)))
  with Res {{
    dbg((use Name).name())
  }}
}}
"#
    );
    let out = to_c(&src).expect("the program compiles");
    // A normal build has neither the table nor a call in `em_alloc`, so it cannot report.
    let quiet = build_and_run(&out_dir("alloc-stats-quiet"), &out, &["-O1"]);
    assert_eq!(quiet.code, 0, "{}", quiet.stderr);
    assert!(
        !quiet.stderr.lines().any(|l| l.starts_with("emela-alloc")),
        "a normal build reports nothing: {:?}",
        quiet.stderr
    );
    let got = build_and_run(
        &out_dir("alloc-stats"),
        &out,
        &["-O1", "-DEMELA_ALLOC_STATS"],
    );
    assert_eq!(got.code, 0, "{}", got.stderr);
    // The program's own output on standard error comes first, then the report, at exit.
    let (head, report) = got.stderr.split_once("\nemela-alloc ").expect("a report");
    let report = format!("emela-alloc {report}");
    assert_eq!(head, "\"9\"\n\"res\"");
    let counts = check_stats(&report);
    let count = |n: &str| counts.iter().find(|(name, _)| name == n).map(|(_, c)| *c);
    assert!(count("cons").is_some(), "a list allocates cons cells");
    assert!(count("String").is_some());
    // The empty evidence tables that `List.fold` builds are the static singleton, not
    // allocations, so no `evidence` line appears.
    assert_eq!(count("evidence"), None);
}

/// An allocation larger than the address-space limit ends the program in `em_out_of_memory`,
/// which reports it and aborts. This fixes what the current implementation does, not a promise of
/// the language: `[dyn.memory]` leaves the behaviour of an allocation failure to the
/// implementation, so only the message and that it ends abnormally are checked here.
#[cfg(all(unix, target_pointer_width = "64"))]
#[test]
fn an_allocation_failure_ends_the_program() {
    use std::os::unix::process::ExitStatusExt;

    if cc().is_none() {
        return;
    }
    // 1 << 37 slots of 8 bytes is 1 TiB, and the product stays within size_t, so that malloc
    // fails instead of the size wrapping.
    //
    // A virtual-memory limit well below the request makes the failure follow from the program
    // rather than from the host: without it, a host with vm.overcommit_memory=1 may let the 1 TiB
    // mapping succeed.
    let src = r#"
#include <sys/resource.h>

#include "emela_runtime.h"

static const uint8_t plain[1] = {0};
static const em_desc huge_desc = {EM_KIND_FIELDS, 1, 0, -1, "huge", NULL, plain};

int main(void) {
  struct rlimit limit = {(rlim_t)256 << 20, (rlim_t)256 << 20};
  if (setrlimit(RLIMIT_AS, &limit) != 0) return 2;
  em_alloc(&huge_desc, (size_t)1 << 37);
  return em_exit(NULL);
}
"#;
    let ran = run_c_output("alloc-out-of-memory", src);
    assert_eq!(
        String::from_utf8_lossy(&ran.stderr),
        "emela: out of memory\n",
        "the message, and nothing else: the leak check does not run on this path"
    );
    assert_eq!(ran.status.signal(), Some(6), "the program aborts");
}
