use super::*;

#[test]
fn join_reuses_completed_results_without_yielding_and_waits_for_discarded_unit() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber}
@external(c, "mark")
fn mark(n: Int) -> () use {}
pub fn work(n: Int) -> Int use Async {
  Fiber.scope(fn() {
    child = Fiber.fork(fn() { mark(2)
      n + 1 })
    mark(1)
    first = Fiber.join(child)
    mark(3)
    Fiber.fork(fn() { mark(99) })
    second = Fiber.join(child)
    mark(4)
    first + second
  })
}
pub fn unit() -> () use Async {
  Fiber.scope(fn() {
    child = Fiber.fork(fn() { mark(1) })
    Fiber.join(child)
    Fiber.join(child)
    mark(2)
  })
}
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    let output = compile(&core, &options().with_heap_bytes(4096)).unwrap();
    assert!(
        output
            .files
            .iter()
            .any(|f| f.contents.contains("#define EMELA_CHILD_WAIT_RUNTIME"))
    );
    assert!(
        !output
            .files
            .iter()
            .any(|f| f.contents.contains("#define EMELA_DEFERRED_CAPACITY"))
    );
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
static int order;
void mark(int32_t n) { assert(n == ++order); }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t result = -1; order = 0;
    EmelaFiber f = emela_export_work_0_start(i, &result);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == 2 * (i + 1) && order == 4); emela_fiber_release(f);
    order = 0; f = emela_export_unit_1_start();
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(order == 2); emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn join_wakes_multiple_waiters_in_fifo_after_the_childs_last_breath() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber}
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
@external(c, "mark")
fn mark(n: Int) -> () use {}
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting: Wait { suspend fn get(n, resume) { resume(wait(n)) } }
pub fn work() -> Int use Async {
  with Waiting {
    Fiber.scope(fn() {
      child = Fiber.fork(fn() { value = (use Wait).get(40)
        mark(4)
        value })
      a = Fiber.fork(fn() { mark(1)
        v = Fiber.join(child)
        mark(5)
        v })
      b = Fiber.fork(fn() { mark(2)
        v = Fiber.join(child)
        mark(6)
        v })
      c = Fiber.fork(fn() { mark(3)
        v = Fiber.join(child)
        mark(7)
        v })
      Fiber.join(a) + Fiber.join(b) + Fiber.join(c)
    })
  }
}
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    compile_and_run(
        compile(&core, &options().with_frame_capacity(40)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
static int order, count;
static EmelaTask task;
void mark(int32_t n) { assert(n == ++order); }
void board_wait_start(EmelaTask t, int32_t n) { assert(n == 40 && count++ == 0); task = t; }
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    order = count = 0; int32_t result = -1;
    EmelaFiber f = emela_export_work_0_start(&result);
    while (emela_scheduler_run(1)) {}
    assert(order == 3 && count == 1 && result == -1 && !emela_fiber_done(f));
    assert(emela_task_complete_int(task, 42));
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == 126 && order == 7); emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn join_results_own_strings_and_lists_after_scope_and_frame_reuse() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber}
pub fn text(n: Int) -> String use Async {
  Fiber.scope(fn() {
    child = Fiber.fork(fn() { Int.to_string(n) })
    first = Fiber.join(child)
    Fiber.join(child)
    first
  })
}

pub fn list(n: Int) -> List[Int] use Async {
  Fiber.scope(fn() {
    child = Fiber.fork(fn() { [n, n + 1] })
    first = Fiber.join(child)
    Fiber.join(child)
    first
  })
}
pub fn sum(xs: List[Int]) -> Int { List.fold(xs, 0, fn(a, b) { a + b }) }
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    compile_and_run(
        compile(&core, &options().with_heap_bytes(4096)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    EmelaString text = {0}; void *list = NULL;
    EmelaFiber f = emela_export_text_0_start(-2147483647 - 1, i % 2 ? &text : NULL);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
    f = emela_export_list_1_start(i, i % 2 ? &list : NULL);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
    if (i % 2) {
      assert(text.owner && text.length == 11 && memcmp(text.data, "-2147483648", 11) == 0);
      assert(emela_export_sum_2(list) == 2 * i + 1);
      emela_embedded_release_string(&text); emela_embedded_release(list);
    }
  }
}
"#,
    );
}

#[test]
fn closing_nested_scopes_unlinks_join_waiters_before_the_target_completes() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
@external(c, "mark")
fn mark(n: Int) -> () use {}
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting: Wait { suspend fn get(n, resume) { resume(wait(n)) } }
pub fn cancel(early: Bool) -> Int use Async {
  with Waiting {
    Fiber.scope(fn() {
      d: Deferred[Int] = (use Scope).deferred()
      target = Fiber.fork(fn() { Deferred.await(d) })
      Fiber.scope(fn() {
        a: Deferred[()] = (use Scope).deferred()
        b: Deferred[()] = (use Scope).deferred()
        Fiber.fork(fn() { Deferred.await(a)
          Fiber.join(target)
          mark(99) })
        Fiber.fork(fn() { Deferred.await(b)
          Fiber.join(target)
          mark(99) })
        Fiber.fork(fn() { Deferred.complete(a, ())
          Deferred.complete(b, ())
          Fiber.join(target)
          mark(99) })
        (use Wait).get(1)
        if early { Deferred.complete(d, 42) } else { False }
      })
      Deferred.complete(d, 42)
      Fiber.join(target)
    })
  }
}
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    compile_and_run(
        compile(
            &core,
            &options().with_frame_capacity(48).with_deferred_capacity(3),
        )
        .unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask pending;
static int called;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t, int32_t n) { assert(n == 1 && !called++); pending = t; }
void mark(int32_t n) { (void)n; abort(); }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t result = -1; called = 0;
    EmelaFiber f = emela_export_cancel_0_start(i % 2, &result);
    while (emela_scheduler_run(1)) {}
    assert(called == 1 && !emela_fiber_done(f));
    assert(emela_task_complete_int(pending, 1));
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == 42 && !emela_task_complete_int(pending, 0));
    emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn leaked_join_callback_rejects_an_ended_scope_after_reusing_execution_slots() {
    let source = r#"
import Std.Fiber.{Fiber}
pub fn leak() -> (fn() -> Int use Async) use Async {
  Fiber.scope(fn() {
    child = Fiber.fork(fn() { 42 })
    Fiber.join(child)
    fn() { Fiber.join(child) }
  })
}
pub fn invoke(f: fn() -> Int fails {} use Async) -> Int use Async { f() }
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    let flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    let ty = flow
        .functions
        .iter()
        .find(|(_, f)| f.name == "leak")
        .unwrap()
        .1
        .ret
        .index();
    compile_and_run(
        compile(&core, &options().with_heap_bytes(4096)).unwrap(),
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static int expected;
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{
  assert(expected && k == EMELA_DEFECT_PANIC && n == 37);
  assert(memcmp(m, "fiber was joined after its scope ended", n) == 0); exit(0);
}}
int main(void) {{
  emela_function_type_{ty} old = {{0}};
  EmelaFiber f = emela_export_leak_0_start(&old);
  while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
  emela_fiber_release(f);
  for (int i = 0; i < 1000; i++) {{
    emela_function_type_{ty} next = {{0}};
    f = emela_export_leak_0_start(&next);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
    emela_embedded_release_function_{ty}(&next);
  }}
  expected = 1; int32_t result = 0; f = emela_export_invoke_1_start(old, &result);
  while (!emela_fiber_done(f)) emela_scheduler_run(1);
  abort();
}}
"#
        ),
    );
}

#[test]
fn join_preserves_int64_float_bits_and_bool() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber}
pub fn wide(n: Int64) -> Int64 use Async {
  Fiber.scope(fn() { child = Fiber.fork(fn() { n })
    Fiber.join(child) })
}
pub fn real(n: Float) -> Float use Async {
  Fiber.scope(fn() { child = Fiber.fork(fn() { n })
    Fiber.join(child) })
}
pub fn flag(n: Bool) -> Bool use Async {
  Fiber.scope(fn() { child = Fiber.fork(fn() { n })
    Fiber.join(child) })
}
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    compile_and_run(
        compile(&core, &options().with_heap_bytes(4096)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int64_t wide = 0; double real = 0, input = i % 2 ? -0.0 : -1.2345678901234567;
    int32_t flag = -1;
    EmelaFiber f = emela_export_wide_0_start(INT64_MIN, &wide);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(wide == INT64_MIN); emela_fiber_release(f);
    f = emela_export_real_1_start(input, &real);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(memcmp(&input, &real, sizeof(double)) == 0); emela_fiber_release(f);
    f = emela_export_flag_2_start(i % 2, &flag);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(flag == i % 2); emela_fiber_release(f);
  }
}
"#,
    );
}
