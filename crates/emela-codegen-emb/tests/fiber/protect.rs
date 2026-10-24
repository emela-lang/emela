use super::*;

#[test]
fn suspend_protect_runs_cleanup_on_yield_return_and_break_without_heap() {
    let mut flow = Module::new(emela_core::Module::new());
    let b = FnBuilder::new(&flow.core, "mark");
    let hook = flow.add(b.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("cleanup_hook".into()),
        }],
    }));
    let call = || Value::Call {
        callee: Callee::Direct(hook),
        args: vec![],
        suspends: false,
        on_fail: None,
    };
    for (name, mode) in [("yielded", 0), ("returned", 1), ("broken", 2)] {
        let mut b = FnBuilder::new(&flow.core, name);
        b.set_exported().set_suspends().set_ret(TypeId::INT);
        let result = b.var("result", TypeId::INT);
        if mode == 0 {
            b.protect(
                Some(result),
                |b| b.term(Term::Yield(Some(Const::Int(42).into()))),
                |b| b.let_(None, call()),
            );
        } else if mode == 1 {
            b.protect(
                None,
                |b| b.term(Term::Return(Some(Const::Int(42).into()))),
                |b| b.let_(None, call()),
            );
        } else {
            b.block(Some(result), "exit", |b, label| {
                b.protect(
                    None,
                    |b| {
                        b.term(Term::Break {
                            label,
                            value: Some(Const::Int(42).into()),
                        })
                    },
                    |b| b.let_(None, call()),
                );
                b.term(Term::Yield(Some(Const::Int(-1).into())));
            });
        }
        b.term(Term::Return(Some(if mode == 1 {
            Const::Int(-1).into()
        } else {
            result.into()
        })));
        flow.add(b.finish());
    }
    compile_and_run(
        compile_flow(&flow, &options().with_heap_bytes(0)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
static int calls;
void cleanup_hook(void) { calls++; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t value = -1; calls = 0;
    EmelaFiber f = emela_export_yielded_1_start(&value);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(value == 42 && calls == 1); emela_fiber_release(f);
    calls = 0; f = emela_export_returned_2_start(&value);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(value == 42 && calls == 1); emela_fiber_release(f);
    calls = 0; f = emela_export_broken_3_start(&value);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(value == 42 && calls == 1); emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn suspend_cleanup_waits_for_an_external_task_before_publishing_the_result() {
    let mut flow = Module::new(emela_core::Module::new());
    let mut b = FnBuilder::new(&flow.core, "wait");
    b.set_suspends();
    let wait = flow.add(b.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("host_wait".into()),
        }],
    }));
    let mut b = FnBuilder::new(&flow.core, "work");
    b.set_exported().set_suspends().set_ret(TypeId::INT);
    b.protect(
        None,
        |b| b.term(Term::Return(Some(Const::Int(42).into()))),
        |b| {
            b.let_(
                None,
                Value::Call {
                    callee: Callee::Direct(wait),
                    args: vec![],
                    suspends: true,
                    on_fail: None,
                },
            );
        },
    );
    b.term(Term::Return(Some(Const::Int(-1).into())));
    flow.add(b.finish());
    compile_and_run(
        compile_flow(&flow, &options().with_heap_bytes(0)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask task;
static int calls;
void board_wait_start(EmelaTask t) { assert(calls++ == 0); task = t; }
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t n) { (void)n; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t value = -1; calls = 0;
    EmelaFiber f = emela_export_work_1_start(&value);
    while (emela_scheduler_run(1)) {}
    assert(calls == 1 && value == -1 && !emela_fiber_done(f));
    assert(emela_task_complete_unit(task));
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(value == 42); emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn scope_cancellation_runs_nested_releases_before_the_parent_continues() {
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
effect FirstResource { fn read() -> Int }
effect SecondResource { fn read() -> Int }
handler Outer: FirstResource { fn read() { 1 }
  release { mark(3) } }
handler Inner: SecondResource { fn read() { 2 }
  release { mark(2) } }
pub fn work() -> Int use Async {
  with Waiting {
    result = Fiber.scope(fn() {
      Fiber.fork(fn() { with Outer, Inner { mark(1)
        (use Wait).get(1) } })
      (use Wait).get(2)
    })
    mark(4)
    result
  }
}
"#,
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
void mark(int32_t n) { assert(n == ++order); }
void board_wait_start(EmelaTask t, int32_t n) { assert(n >= 1 && n <= 2); tasks[n - 1] = t; count++; }
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t n) { (void)n; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t value = -1; count = order = 0;
    EmelaFiber f = emela_export_work_0_start(&value);
    while (emela_scheduler_run(1)) {}
    assert(count == 2 && order == 1);
    assert(emela_task_complete_int(tasks[1], 42));
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(value == 42 && order == 4 && !emela_task_complete_int(tasks[0], 0));
    emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn cancellation_preserves_a_suspending_release_and_its_pending_task() {
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
effect Marker { fn read() -> Int }
handler Resource: Marker { fn read() { 1 }
  release { mark(2)
    (use Wait).get(3)
    mark(3) } }
pub fn work() -> Int use Async {
  with Waiting {
    result = Fiber.scope(fn() {
      Fiber.fork(fn() { with Resource { mark(1)
        (use Wait).get(1) } })
      (use Wait).get(2)
    })
    mark(4)
    result
  }
}
"#,
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
static EmelaTask tasks[3];
static int count, order;
void mark(int32_t n) { assert(n == ++order); }
void board_wait_start(EmelaTask t, int32_t n) { assert(n >= 1 && n <= 3); tasks[n - 1] = t; count++; }
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t n) { (void)n; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t value = -1; count = order = 0;
    EmelaFiber f = emela_export_work_0_start(&value);
    while (emela_scheduler_run(1)) {}
    assert(count == 2 && order == 1);
    if (i % 2) {
      assert(emela_task_complete_int(tasks[0], 1));
      while (emela_scheduler_run(1)) {}
      assert(count == 3 && order == 2);
    }
    assert(emela_task_complete_int(tasks[1], 42));
    while (emela_scheduler_run(1)) {}
    assert(count == 3 && order == 2 && !emela_fiber_done(f));
    assert(!emela_task_complete_int(tasks[0], 0));
    assert(emela_task_complete_int(tasks[2], 3));
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(value == 42 && order == 4); emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn a_root_waits_for_cleanup_of_its_resumed_operation_tail() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
@external(c, "mark")
fn mark(n: Int) -> () use {}
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting: Wait { suspend fn get(n, resume) { resume(wait(n)) } }
effect Marker { fn read() -> Int }
handler Resource: Marker { fn read() { 1 }
  release { mark(3)
    (use Wait).get(2)
    mark(4) } }
effect Gate { suspend fn go() -> Int }
handler Gated: Gate {
  suspend fn go(resume) { with Resource { mark(1)
    resume(7)
    (use Wait).get(1)
    () } }
}
pub fn work() -> Int use Async {
  with Waiting, Gated { result = (use Gate).go()
    mark(2)
    result }
}
"#,
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
void mark(int32_t n) { assert(n == ++order); }
void board_wait_start(EmelaTask t, int32_t n) { assert(n >= 1 && n <= 2); tasks[n - 1] = t; count++; }
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t n) { (void)n; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t value = -1; count = order = 0;
    EmelaFiber f = emela_export_work_0_start(&value);
    while (emela_scheduler_run(1)) {}
    assert(order == 3 && count == 2 && !emela_fiber_done(f));
    assert(!emela_task_complete_int(tasks[0], 0));
    assert(emela_task_complete_int(tasks[1], 2));
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(value == 7 && order == 4); emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn defects_in_suspend_protect_keep_the_configured_immediate_stop_policy() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
@external(c, "mark")
fn mark(n: Int) -> () use {}
effect Marker { fn read() -> Int }
handler Resource: Marker { fn read() { 1 }
  release { mark(99) } }
pub fn work() -> Int use Async { with Resource { panic("bad") } }
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    compile_and_run(
        compile(&core, &options()).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
void mark(int32_t n) { (void)n; abort(); }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {
  assert(k == EMELA_DEFECT_PANIC && n == 3 && memcmp(m, "bad", n) == 0); exit(0);
}
int main(void) {
  int32_t value = -1; EmelaFiber f = emela_export_work_0_start(&value);
  while (!emela_fiber_done(f)) emela_scheduler_run(1);
  abort();
}
"#,
    );
}
