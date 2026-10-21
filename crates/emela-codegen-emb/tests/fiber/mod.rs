use super::*;

mod aggregate;
mod aggregate_abi;
mod attempt;
mod await_;
mod cause;
mod child_reclaim;
mod failure;
mod function_aggregate;
mod integration;
mod interrupt;
mod join;
mod notes;
mod overlap;
mod payload;
mod protect;
mod task_failure;

fn options() -> Options {
    Options::default()
        .with_heap_bytes(8192)
        .with_fiber_capacity(8)
        .with_frame_capacity(24)
        .with_frame_bytes(512)
        .with_task_capacity(2)
        .with_event_capacity(2)
        .with_task_adapter("host_wait", "board_wait_start")
}

#[test]
fn scope_starts_children_at_real_suspension_and_reclaims_waiting_tasks() {
    let core = support::try_lower(
        &[(
            "main.emel",
            include_str!("../../examples/support/fiber_scope.emel"),
        )],
        Roots::Exported,
    )
    .unwrap();
    compile_and_run(
        compile(&core, &options()).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask tasks[2];
static int count, order;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "defect %d: %.*s\n", k, n, m); abort(); }
void board_wait_start(EmelaTask t, int32_t n) { assert(count < 2); assert(n == 40 + count); tasks[count++] = t; }
void mark(int32_t n) { assert(n == ++order); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t value = -1;
    count = order = 0;
    EmelaFiber f = emela_export_work_0_start(40, &value);
    assert(emela_scheduler_run(0) == 0 && count == 0 && order == 0);
    while (count != 2) assert(emela_scheduler_run(1));
    assert(order == 2 && value == -1);
    assert(emela_task_complete_int(tasks[0], 42));
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(order == 3 && value == 42);
    assert(!emela_task_complete_int(tasks[1], 99));
    emela_fiber_release(f);
  }
  return 0;
}

"#,
    );
}

#[test]
fn scope_cancels_unstarted_children_before_running_their_function() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Scope}
@external(c, "mark")
fn mark(n: Int) -> () use {}
pub fn skip() -> Int use Async {
  Fiber.scope(fn() {
    Fiber.fork(fn() {
      mark(99)
      1
    })
    mark(1)
    7
  })
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
#include "emela_embedded.h"
#include "emela_board.h"
static int marks;
void mark(int32_t n) { assert(n == 1); marks++; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t result = 0;
    EmelaFiber f = emela_export_skip_0_start(&result);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == 7 && marks == i + 1);
    emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn completed_children_and_queued_events_do_not_leak_across_scope_reuse() {
    let core = support::try_lower(
        &[(
            "main.emel",
            include_str!("../../examples/support/fiber_scope.emel"),
        )],
        Roots::Exported,
    )
    .unwrap();
    compile_and_run(
        compile(&core, &options()).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask tasks[2];
static int count, order, mode;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
void board_wait_start(EmelaTask t, int32_t n) { (void)n; assert(count < 2); tasks[count++] = t; }
void mark(int32_t n) {
  assert(n == ++order);
  if (n == 3 && mode) assert(emela_task_complete_int(tasks[1], 99));
}
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t value = -1;
    count = order = 0; mode = i % 2;
    EmelaFiber f = emela_export_work_0_start(40, &value);
    while (count != 2) assert(emela_scheduler_run(1));
    if (!mode) {
      assert(emela_task_complete_int(tasks[1], 99));
      while (emela_scheduler_run(1)) {}
      assert(!emela_fiber_done(f) && value == -1);
    }
    assert(emela_task_complete_int(tasks[0], 42));
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(value == 42 && order == 3);
    assert(!emela_task_complete_int(tasks[1], 99));
    emela_fiber_release(f);
    while (emela_scheduler_run(1)) {}
  }
}
"#,
    );
}

#[test]
fn closing_outer_scope_reclaims_nested_scopes_and_their_tasks() {
    let source = r#"
import Std.Fiber.{Fiber, Scope}
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
@external(c, "mark")
fn mark(n: Int) -> () use {}
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting: Wait {
  suspend fn get(n, resume) { resume(wait(n)) }
}
pub fn nested() -> Int use Async {
  with Waiting {
    Fiber.scope(fn() {
      Fiber.fork(fn() {
        Fiber.scope(fn() {
          Fiber.fork(fn() {
            mark(3)
            (use Wait).get(3)
          })
          mark(2)
          (use Wait).get(2)
        })
      })
      mark(1)
      (use Wait).get(1)
    })
  }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    compile_and_run(
        compile(
            &core,
            &options()
                .with_heap_bytes(16384)
                .with_task_capacity(3)
                .with_event_capacity(3),
        )
        .unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask tasks[3];
static int count, order;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
void board_wait_start(EmelaTask t, int32_t n) { assert(n == ++count); tasks[n - 1] = t; }
void mark(int32_t n) { assert(n == ++order); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t result = 0; count = order = 0;
    EmelaFiber f = emela_export_nested_0_start(&result);
    while (count < 3) assert(emela_scheduler_run(1));
    assert(emela_task_complete_int(tasks[0], 42));
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == 42 && order == 3);
    assert(!emela_task_complete_int(tasks[1], 0));
    assert(!emela_task_complete_int(tasks[2], 0));
    emela_fiber_release(f);
    while (emela_scheduler_run(1)) {}
  }
}
"#,
    );
}

#[test]
fn children_can_fork_siblings_and_inherit_the_scope_call_evidence() {
    let source = r#"
import Std.Fiber.{Fiber, Scope}
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
@external(c, "mark")
fn mark(n: Int) -> () use {}
effect Config { fn read() -> Int }
handler Outer: Config { fn read() { 10 } }
handler Inner: Config { fn read() { 90 } }
fn read() -> Int use Config { (use Config).read() }
fn branch() -> () use { Config, Scope[{}, {Config}] } {
  mark(read())
  Fiber.fork(fn() { mark(read() + 1) })
  ()
}
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting: Wait {
  suspend fn get(n, resume) { resume(wait(n)) }
}
pub fn siblings() -> Int use Async {
  with Outer {
    with Waiting {
      Fiber.scope(fn() {
        with Inner {
          Fiber.fork(branch)
        }
        (use Wait).get(1)
      })
    }
  }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    compile_and_run(
        compile(&core, &options().with_heap_bytes(16384)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask task;
static int marks;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
void board_wait_start(EmelaTask t, int32_t n) { assert(n == 1); task = t; }
void mark(int32_t n) { assert(n == 10 + marks++); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t result = 0; marks = 0;
    EmelaFiber f = emela_export_siblings_0_start(&result);
    while (emela_scheduler_run(1)) {}
    assert(marks == 2 && !emela_fiber_done(f));
    assert(emela_task_complete_int(task, 42));
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == 42);
    emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn scope_handles_keep_closed_state_after_frame_and_pool_reuse() {
    let mut flow = Module::new(emela_core::Module::new());
    let body_type = flow.core.types.intern(Type::Fn(FnType {
        suspends: true,
        ..FnType::pure(vec![TypeId::FIBER_SCOPE], TypeId::FIBER_SCOPE)
    }));
    let child_type = flow.core.types.intern(Type::Fn(FnType {
        suspends: true,
        ..FnType::pure(vec![], TypeId::ANY)
    }));
    let mut b = FnBuilder::new(&flow.core, "body");
    b.set_suspends().set_ret(TypeId::FIBER_SCOPE);
    let scope = b.param("scope", TypeId::FIBER_SCOPE);
    b.term(Term::Return(Some(scope.into())));
    let body = flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "child");
    b.set_suspends().set_ret(TypeId::ANY);
    b.term(Term::Return(Some(Const::String(String::new()).into())));
    let child = flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "closed");
    b.set_exported().set_suspends().set_ret(TypeId::FIBER_SCOPE);
    let body = b.assign("body", body_type, Value::FnRef(body));
    let scope = b.assign(
        "scope",
        TypeId::FIBER_SCOPE,
        Value::Fiber {
            op: emela_core::FiberOp::Scope { fails: false },
            args: vec![body.into()],
            on_fail: None,
        },
    );
    b.term(Term::Return(Some(scope.into())));
    flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "invalid");
    b.set_exported().set_ret(TypeId::ANY);
    let scope = b.param("scope", TypeId::FIBER_SCOPE);
    let child = b.assign("child", child_type, Value::FnRef(child));
    let result = b.assign(
        "result",
        TypeId::ANY,
        Value::Fiber {
            op: emela_core::FiberOp::Fork,
            args: vec![scope.into(), child.into()],
            on_fail: None,
        },
    );
    b.term(Term::Return(Some(result.into())));
    flow.add(b.finish());
    compile_and_run(
        compile_flow(&flow, &options().with_heap_bytes(2048)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static int expect_closed;
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {
  assert(expect_closed && k == EMELA_DEFECT_PANIC && n == 34);
  assert(memcmp(m, "fiber scope was used after it ended", 34) == 0);
  exit(0);
}
int main(void) {
  void *old = NULL;
  EmelaFiber first = emela_export_closed_2_start(&old);
  while (!emela_fiber_done(first)) assert(emela_scheduler_run(1));
  emela_fiber_release(first);
  for (int i = 0; i < 1000; i++) {
    void *next = NULL;
    EmelaFiber f = emela_export_closed_2_start(&next);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(next != old);
    emela_fiber_release(f);
    emela_embedded_release(next);
  }
  expect_closed = 1;
  (void)emela_export_invalid_3(old);
  abort();
}
"#,
    );
}

#[test]
fn fork_exhaustion_is_a_fixed_pool_defect_before_children_start() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber}
@external(c, "mark")
fn mark(n: Int) -> () use {}
pub fn full() -> Int use Async {
  Fiber.scope(fn() {
    Fiber.fork(fn() { mark(99) })
    Fiber.fork(fn() { mark(99) })
    7
  })
}
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    compile_and_run(
        compile(&core, &options().with_fiber_capacity(2)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
void mark(int32_t n) { (void)n; abort(); }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)m; (void)n; assert(k == EMELA_DEFECT_HEAP_EXHAUSTED); exit(0); }
int main(void) {
  EmelaFiber f = emela_export_full_0_start(NULL);
  while (!emela_fiber_done(f)) emela_scheduler_run(1);
  abort();
}
"#,
    );
}

#[test]
fn scope_keeps_managed_captures_and_boxes_reference_results_without_leaks() {
    let source = r#"
import Std.Fiber.{Fiber}
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
@external(c, "mark")
fn mark(n: Int) -> () use {}
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting: Wait {
  suspend fn get(n, resume) { resume(wait(n)) }
}
pub fn managed(n: Int) -> String use Async {
  with Waiting {
    text = Int.to_string(n)
    xs = [n, n + 1]
    Fiber.scope(fn() {
      Fiber.fork(fn() {
        mark(String.length(text))
        (use Wait).get(n + 1)
        xs
      })
      (use Wait).get(n)
      text
    })
  }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    compile_and_run(
        compile(&core, &options().with_heap_bytes(8192)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask tasks[2];
static int count, marks;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
void board_wait_start(EmelaTask t, int32_t n) { assert(n == 40 + count); tasks[count++] = t; }
void mark(int32_t n) { assert(n == 2); marks++; }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    EmelaString result = {0}; count = marks = 0;
    EmelaFiber f = emela_export_managed_0_start(40, i % 3 ? &result : NULL);
    while (count < 2) assert(emela_scheduler_run(1));
    if (i % 2) {
      assert(emela_task_complete_int(tasks[1], 41));
      while (emela_scheduler_run(1)) {}
      assert(!emela_fiber_done(f));
    }
    assert(emela_task_complete_int(tasks[0], 40));
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(marks == 1);
    emela_fiber_release(f);
    if (i % 3) { assert(result.owner && result.length == 2 && memcmp(result.data, "40", 2) == 0); emela_embedded_release_string(&result); }
    assert(!emela_task_complete_int(tasks[1], 0));
  }
}
"#,
    );
}
