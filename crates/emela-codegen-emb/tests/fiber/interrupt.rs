use super::*;

#[test]
fn interrupt_cancels_unstarted_children_and_does_not_yield_for_ended_children() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber}
@external(c, "mark")
fn mark(n: Int) -> () use {}
pub fn work() -> Int use Async {
  Fiber.scope(fn() {
    skipped = Fiber.fork(fn() { mark(99) })
    Fiber.interrupt(skipped)
    Fiber.interrupt(skipped)
    done = Fiber.fork(fn() { 42 })
    value = Fiber.join(done)
    Fiber.fork(fn() { mark(99) })
    Fiber.interrupt(done)
    mark(1)
    value
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
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
static int order;
void mark(int32_t n) { assert(n == ++order); }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t value = -1; order = 0;
    EmelaFiber f = emela_export_work_0_start(&value);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(value == 42 && order == 1); emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn interrupt_waits_for_suspending_cleanup_and_ignores_old_task_notifications() {
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
  release { mark(3)
    (use Wait).get(3)
    mark(4) } }
pub fn work() -> Int use Async {
  with Waiting {
    Fiber.scope(fn() {
      child = Fiber.fork(fn() { with Resource { mark(1)
        (use Wait).get(1) } })
      result = (use Wait).get(2)
      mark(2)
      Fiber.interrupt(child)
      mark(5)
      Fiber.interrupt(child)
      result
    })
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
static int count, order, queue_old;
void mark(int32_t n) { assert(n == ++order); if (n == 2 && queue_old) assert(emela_task_complete_int(tasks[0], 99)); }
void board_wait_start(EmelaTask t, int32_t n) { assert(n >= 1 && n <= 3); tasks[n - 1] = t; count++; }
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t n) { (void)n; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t value = -1; count = order = 0; queue_old = i % 2;
    EmelaFiber f = emela_export_work_0_start(&value);
    while (emela_scheduler_run(1)) {}
    assert(count == 2 && order == 1);
    assert(emela_task_complete_int(tasks[1], 42));
    while (emela_scheduler_run(1)) {}
    assert(count == 3 && order == 3 && !emela_fiber_done(f));
    assert(!emela_task_complete_int(tasks[0], 0));
    assert(emela_task_complete_int(tasks[2], 3));
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(value == 42 && order == 5); emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn multiple_interrupt_waiters_resume_fifo_after_release_finishes() {
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
  release { mark(5)
    (use Wait).get(2)
    mark(6) } }
pub fn work() -> Int use Async {
  with Waiting {
    Fiber.scope(fn() {
      child = Fiber.fork(fn() { with Resource { mark(1)
        (use Wait).get(1) } })
      a = Fiber.fork(fn() { mark(2)
        Fiber.interrupt(child)
        mark(7) })
      b = Fiber.fork(fn() { mark(3)
        Fiber.interrupt(child)
        mark(8) })
      c = Fiber.fork(fn() { mark(4)
        Fiber.interrupt(child)
        mark(9) })
      Fiber.join(a)
      Fiber.join(b)
      Fiber.join(c)
      42
    })
  }
}
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    compile_and_run(
        compile(&core, &options().with_frame_capacity(48)).unwrap(),
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
    assert(count == 2 && order == 5 && !emela_fiber_done(f));
    assert(!emela_task_complete_int(tasks[0], 0));
    assert(emela_task_complete_int(tasks[1], 2));
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(value == 42 && order == 9); emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn operation_tails_do_not_keep_a_copy_of_an_expired_cleanup_mask() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting: Wait { suspend fn get(n, resume) { resume(wait(n)) } }
effect Finish { suspend fn go() -> () }
handler Finishing: Finish { suspend fn go(resume) {
  resume(())
  (use Wait).get(1)
  ()
} }
effect Marker { fn read() -> Int }
handler Resource: Marker { fn read() { 1 }
  release { (use Finish).go() } }
pub fn work() -> Int use Async { with Waiting, Finishing, Resource { 42 } }
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
static EmelaTask task;
static int calls;
void board_wait_start(EmelaTask t, int32_t n) { assert(n == 1 && calls++ == 0); task = t; }
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t n) { (void)n; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t value = -1; calls = 0;
    EmelaFiber f = emela_export_work_0_start(&value);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(value == 42 && calls == 1 && !emela_task_complete_int(task, 0)); emela_fiber_release(f);
  }
}
"#,
    );
}

// Move an ordinary source call into a Flow IR cleanup while preserving its capture operands.
pub(super) fn protect_cleanup_call(flow: &mut Module) {
    let cleanup = flow
        .functions
        .iter()
        .find(|(_, f)| f.name == "cleanup")
        .unwrap()
        .0;
    let mut count = 0;
    for (_, function) in flow.functions.iter_mut() {
        let sequences: Vec<_> = function
            .seqs
            .iter()
            .map(|(id, seq)| (id, seq.clone()))
            .collect();
        for (id, seq) in sequences {
            let Some(position) = seq.stmts.iter().position(|&id| matches!(&function.stmts[id],
                emela_lir::Stmt::Let { value: Value::Call { callee: Callee::Direct(target), .. }, .. } if *target == cleanup)) else { continue };
            assert_eq!(position + 1, seq.stmts.len());
            let mut required = std::collections::HashSet::new();
            let emela_lir::Stmt::Let { value, .. } = &function.stmts[seq.stmts[position]] else {
                unreachable!()
            };
            value.for_each_var(|var| {
                required.insert(var);
            });
            let mut prefix = std::collections::HashSet::new();
            for &id in seq.stmts[..position].iter().rev() {
                if let emela_lir::Stmt::Let {
                    dst: Some(dst),
                    value,
                    ..
                } = &function.stmts[id]
                    && required.contains(dst)
                {
                    assert!(matches!(
                        value,
                        Value::EvExtend { .. } | Value::EvLookup { .. }
                    ));
                    prefix.insert(id);
                    value.for_each_var(|var| {
                        required.insert(var);
                    });
                }
            }
            let body = function.seqs.alloc(emela_lir::Seq {
                stmts: seq.stmts[..position]
                    .iter()
                    .copied()
                    .filter(|id| !prefix.contains(id))
                    .collect(),
                term: seq.term,
            });
            let cleanup = function.seqs.alloc(emela_lir::Seq {
                stmts: vec![seq.stmts[position]],
                term: Term::Yield(None),
            });
            let (dst, term) = match &function.seqs[body].term {
                Term::Yield(Some(value)) => {
                    let ty = match value {
                        Operand::Var(v) => function.vars[*v].ty,
                        Operand::Const(Const::Int(_)) => TypeId::INT,
                        _ => panic!("unsupported cleanup fixture result"),
                    };
                    let dst = function.vars.alloc(emela_lir::VarData {
                        hint: "protected_result".into(),
                        ty,
                        repr: emela_lir::repr_of(&flow.core, ty),
                    });
                    (Some(dst), Term::Yield(Some(dst.into())))
                }
                Term::Yield(None) => (None, Term::Yield(None)),
                _ => (None, Term::Unreachable),
            };
            let protect = function
                .stmts
                .alloc(emela_lir::Stmt::Protect { dst, body, cleanup });
            function.seqs[id] = emela_lir::Seq {
                stmts: seq.stmts[..position]
                    .iter()
                    .copied()
                    .filter(|id| prefix.contains(id))
                    .chain(std::iter::once(protect))
                    .collect(),
                term,
            };
            count += 1;
        }
    }
    assert_eq!(count, 1);
}

#[test]
fn self_interrupt_unwinds_the_body_and_is_deferred_inside_cleanup() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}
@external(c, "mark")
fn mark(n: Int) -> () use {}
fn cleanup(me: Deferred[Fiber[Int]], done: Deferred[()], inside: Bool) -> () use Async {
  mark(2)
  if inside { Fiber.interrupt(Deferred.await(me)) } else { () }
  mark(3)
  Deferred.complete(done, ())
  ()
}
pub fn work(inside: Bool) -> Int use Async {
  Fiber.scope(fn() {
    me: Deferred[Fiber[Int]] = (use Scope).deferred()
    done: Deferred[()] = (use Scope).deferred()
    child = Fiber.fork(fn() {
      mark(1)
      if inside { () } else { Fiber.interrupt(Deferred.await(me))
        mark(99) }
      cleanup(me, done, inside)
      7
    })
    Deferred.complete(me, child)
    Deferred.await(done)
    Fiber.interrupt(child)
    mark(4)
    42
  })
}
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    let mut flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    protect_cleanup_call(&mut flow);
    let id = u32::from(
        flow.functions
            .iter()
            .find(|(_, f)| f.name == "work")
            .unwrap()
            .0
            .into_raw(),
    );
    compile_and_run(
        compile_flow(&flow, &options().with_deferred_capacity(2)).unwrap(),
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
static int order;
void mark(int32_t n) {{ assert(n == ++order); }}
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }}
int main(void) {{
  for (int i = 0; i < 1000; i++) {{
    int32_t value = -1; order = 0;
    EmelaFiber f = emela_export_work_{id}_start(i % 2, &value);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(value == 42 && order == 4); emela_fiber_release(f);
  }}
}}
"#
        ),
    );
}

#[test]
fn interrupted_cleanup_can_wait_on_a_deferred_completed_by_a_sibling() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}
@external(c, "mark")
fn mark(n: Int) -> () use {}
fn cleanup(started: Deferred[()], done: Deferred[()]) -> () use Async {
  mark(1)
  Deferred.complete(started, ())
  Deferred.await(done)
  mark(3)
}
pub fn work() -> Int use Async {
  Fiber.scope(fn() {
    stop: Deferred[Int] = (use Scope).deferred()
    started: Deferred[()] = (use Scope).deferred()
    done: Deferred[()] = (use Scope).deferred()
    child = Fiber.fork(fn() { Deferred.await(stop)
      cleanup(started, done)
      7 })
    canceller = Fiber.fork(fn() { Fiber.interrupt(child) })
    completer = Fiber.fork(fn() { Deferred.await(started)
      mark(2)
      Deferred.complete(done, ()) })
    Fiber.join(canceller)
    Fiber.join(completer)
    mark(4)
    42
  })
}
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    let mut flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    protect_cleanup_call(&mut flow);
    let id = u32::from(
        flow.functions
            .iter()
            .find(|(_, f)| f.name == "work")
            .unwrap()
            .0
            .into_raw(),
    );
    compile_and_run(
        compile_flow(
            &flow,
            &options().with_deferred_capacity(3).with_frame_capacity(48),
        )
        .unwrap(),
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
static int order;
void mark(int32_t n) {{ assert(n == ++order); }}
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }}
int main(void) {{
  for (int i = 0; i < 1000; i++) {{
    int32_t value = -1; order = 0;
    EmelaFiber f = emela_export_work_{id}_start(&value);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(value == 42 && order == 4); emela_fiber_release(f);
  }}
}}
"#
        ),
    );
}

#[test]
fn join_of_an_interrupted_child_is_the_defined_panic_defect() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber}
pub fn work() -> Int use Async {
  Fiber.scope(fn() { child = Fiber.fork(fn() { 42 })
    Fiber.interrupt(child)
    Fiber.join(child) })
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
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {
  assert(k == EMELA_DEFECT_PANIC && n == 21 && memcmp(m, "fiber was interrupted", n) == 0); exit(0);
}
int main(void) {
  int32_t value = -1; EmelaFiber f = emela_export_work_0_start(&value);
  while (!emela_fiber_done(f)) emela_scheduler_run(1);
  abort();
}
"#,
    );
}

#[test]
fn leaked_interrupt_callback_reports_ended_scope_after_slot_reuse() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber}
pub fn leak() -> (fn() -> () use Async) use Async {
  Fiber.scope(fn() { child = Fiber.fork(fn() { 42 })
    fn() { Fiber.interrupt(child) } })
}
pub fn invoke(f: fn() -> () fails {} use Async) -> () use Async { f() }
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
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
  assert(expected && k == EMELA_DEFECT_PANIC && n == 42);
  assert(memcmp(m, "fiber was interrupted after its scope ended", n) == 0); exit(0);
}}
int main(void) {{
  emela_function_type_{ty} old = {{0}};
  EmelaFiber f = emela_export_leak_0_start(&old);
  while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
  emela_fiber_release(f);
  for (int i = 0; i < 1000; i++) {{
    emela_function_type_{ty} next = {{0}}; f = emela_export_leak_0_start(&next);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f); emela_embedded_release_function_{ty}(&next);
  }}
  expected = 1; f = emela_export_invoke_1_start(old);
  while (!emela_fiber_done(f)) emela_scheduler_run(1);
  abort();
}}
"#
        ),
    );
}

#[test]
fn await_of_interrupted_children_returns_owned_failures_repeatedly_and_for_unit() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Success, Failure}
pub fn work() -> Bool use Async {
  Fiber.scope(fn() {
    child = Fiber.fork(fn() { 42 })
    Fiber.interrupt(child)
    first = Fiber.await(child) match { Failure(_) -> True
      Success(_) -> False }
    Fiber.await(child)
    second = Fiber.await(child) match { Failure(_) -> True
      Success(_) -> False }
    unit = Fiber.fork(fn() { () })
    Fiber.interrupt(unit)
    third = Fiber.await(unit) match { Failure(_) -> True
      Success(_) -> False }
    first && second && third
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
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t value = -1; EmelaFiber f = emela_export_work_0_start(i % 2 ? &value : NULL);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    if (i % 2) assert(value == 1);
    emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn await_waiters_receive_failure_after_interrupt_cleanup_before_interrupt_waiters() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Scope, Success, Failure}
import Std.Deferred.{Deferred}
@external(c, "mark")
fn mark(n: Int) -> () use {}
effect Marker { fn read() -> Int }
handler Resource: Marker { fn read() { 1 }
  release { mark(4) } }
pub fn work() -> Bool use Async {
  Fiber.scope(fn() {
    stop: Deferred[Int] = (use Scope).deferred()
    child = Fiber.fork(fn() { with Resource { mark(1)
      Deferred.await(stop) } })
    a = Fiber.fork(fn() { mark(2)
      result = Fiber.await(child) match { Failure(_) -> True
        Success(_) -> False }
      mark(5)
      result })
    b = Fiber.fork(fn() { mark(3)
      Fiber.interrupt(child)
      mark(6) })
    result = Fiber.join(a)
    Fiber.join(b)
    result
  })
}
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    compile_and_run(
        compile(
            &core,
            &options().with_deferred_capacity(1).with_frame_capacity(40),
        )
        .unwrap(),
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
    int32_t value = -1; order = 0;
    EmelaFiber f = emela_export_work_0_start(&value);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(value == 1 && order == 6); emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn interruption_causes_survive_scope_and_heap_reuse_and_can_be_released_by_the_caller() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Success, Failure}
import Std.Cause.{Cause}
pub fn failure() -> Cause[{}] use Async {
  Fiber.scope(fn() {
    child = Fiber.fork(fn() { 42 })
    Fiber.interrupt(child)
    Fiber.await(child) match { Failure(c) -> c
      Success(_) -> panic("expected interruption") }
  })
}
pub fn keep(c: Cause[{}]) -> Cause[{}] { c }
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
  void *old = NULL;
  EmelaFiber f = emela_export_failure_0_start(&old);
  while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
  emela_fiber_release(f);
  void *same = emela_export_keep_1(old); assert(same == old); emela_embedded_release(same);
  for (int i = 0; i < 1000; i++) {
    void *next = NULL; uint32_t kind;
    f = emela_export_failure_0_start(i % 2 ? &next : NULL);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
    memcpy(&kind, old, sizeof(kind)); assert(kind == 2);
    if (i % 2) { assert(next != old); memcpy(&kind, next, sizeof(kind)); assert(kind == 2); emela_embedded_release(next); }
  }
  emela_embedded_release(old);
}
"#,
    );
}

#[test]
fn interrupted_cleanup_can_close_a_nested_scope_and_wait_for_its_child_release() {
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
effect Marker { fn read() -> Int }
handler Resource: Marker { fn read() { 1 }
  release { (use Wait).get(3)
    mark(3) } }
fn cleanup() -> () use {Async, Wait} {
  Fiber.scope(fn() {
    Fiber.fork(fn() { with Resource { mark(2)
      (use Wait).get(1) } })
    (use Wait).get(2)
  })
  mark(4)
}
pub fn work() -> Int use Async {
  with Waiting {
    Fiber.scope(fn() {
      stop: Deferred[Int] = (use Scope).deferred()
      child = Fiber.fork(fn() { mark(1)
        Deferred.await(stop)
        cleanup()
        7 })
      canceller = Fiber.fork(fn() { Fiber.interrupt(child) })
      Fiber.join(canceller)
      mark(5)
      42
    })
  }
}
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    let mut flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    protect_cleanup_call(&mut flow);
    let id = u32::from(
        flow.functions
            .iter()
            .find(|(_, f)| f.name == "work")
            .unwrap()
            .0
            .into_raw(),
    );
    compile_and_run(
        compile_flow(
            &flow,
            &options().with_deferred_capacity(1).with_frame_capacity(48),
        )
        .unwrap(),
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask tasks[3];
static int count, order;
void mark(int32_t n) {{ assert(n == ++order); }}
void board_wait_start(EmelaTask t, int32_t n) {{ assert(n >= 1 && n <= 3); tasks[n - 1] = t; count++; }}
uint32_t emela_board_event_lock(void) {{ return 0; }}
void emela_board_event_unlock(uint32_t n) {{ (void)n; }}
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }}
int main(void) {{
  for (int i = 0; i < 1000; i++) {{
    int32_t value = -1; count = order = 0;
    EmelaFiber f = emela_export_work_{id}_start(&value);
    while (emela_scheduler_run(1)) {{}}
    assert(count == 2 && order == 2 && !emela_fiber_done(f));
    assert(emela_task_complete_int(tasks[1], 2));
    while (emela_scheduler_run(1)) {{}}
    assert(count == 3 && order == 2 && !emela_task_complete_int(tasks[0], 0));
    assert(emela_task_complete_int(tasks[2], 3));
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(value == 42 && order == 5); emela_fiber_release(f);
  }}
}}
"#
        ),
    );
}
