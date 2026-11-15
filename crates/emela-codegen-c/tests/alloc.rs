//! The allocator of the C runtime: size-class bins for fixed-size objects.
//!
//! The bins are checked from C, because what they guarantee (a block of one size class coming
//! back for the next object of that class, and not for one of another class) is not visible from
//! Emela.

mod common;

use std::process::Command;
use std::time::Duration;

use common::{cc, out_dir, run_with_timeout};
use emela_codegen_c::{
    EMELA_H, EMELA_H_FILE, RUNTIME_C, RUNTIME_C_FILE, RUNTIME_H, RUNTIME_H_FILE,
};

/// Checks the bins. It allocates and frees through the runtime itself, then ends with `em_exit`,
/// whose leak check (the runtime is compiled with `-DEMELA_LEAK_CHECK`) must find nothing live.
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

/// Builds a C program against the runtime and runs it. The program ends with `em_exit`, so the
/// leak check is part of what it checks.
fn run_c(name: &str, main_c: &str) -> (i32, String, String) {
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
    command
        .env_remove("EMELA_ALLOC_STATS")
        .env_remove("EMELA_LEAK_CHECK")
        .current_dir(&dir);
    let ran = run_with_timeout(command, Duration::from_secs(120)).expect("the program timed out");
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
