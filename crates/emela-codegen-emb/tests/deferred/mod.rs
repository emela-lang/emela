use super::*;

fn options() -> Options {
    Options::default()
        .with_heap_bytes(8192)
        .with_fiber_capacity(8)
        .with_frame_capacity(24)
        .with_frame_bytes(512)
        .with_deferred_capacity(2)
}

#[test]
fn deferred_constructor_reuses_slots_and_scope_only_programs_omit_the_pool() {
    let core = support::try_lower(
        &[(
            "main.emel",
            "import Std.Fiber.{Fiber, Scope}\nimport Std.Deferred.{Deferred}\npub fn work() -> () use Async {\n  Fiber.scope(fn() {\n    d: Deferred[Int] = (use Scope).deferred()\n    ()\n  })\n}\n",
        )],
        Roots::Exported,
    )
    .unwrap();
    let output = compile(&core, &options().with_deferred_capacity(1)).unwrap();
    assert!(output.files.iter().any(|f| f.path == "emela_embedded.c"
        && f.contents.contains("#define EMELA_DEFERRED_CAPACITY 1")));
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    EmelaFiber f = emela_export_work_0_start();
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
  }
}
"#,
    );
    assert!(
        compile(&core, &options().with_deferred_capacity(0))
            .unwrap_err()
            .iter()
            .any(|e| e.kind == ErrorKind::InvalidOptions)
    );
    let scope = support::try_lower(&[("main.emel", "import Std.Fiber.{Fiber}\npub fn work() -> Int use Async { Fiber.scope(fn() { 1 }) }\n")], Roots::Exported).unwrap();
    let output = compile(&scope, &options().with_deferred_capacity(0)).unwrap();
    assert!(
        !output.files.iter().any(|f| f.path == "emela_embedded.c"
            && f.contents.contains("#define EMELA_DEFERRED_CAPACITY"))
    );
}

#[test]
fn discarded_deferreds_reuse_one_slot_within_a_long_lived_scope() {
    let mut flow = Module::new(emela_core::Module::new());
    let ft = flow.core.types.intern(Type::Fn(FnType {
        suspends: true,
        ..FnType::pure(vec![TypeId::FIBER_SCOPE], TypeId::UNIT)
    }));
    let mut b = FnBuilder::new(&flow.core, "body");
    b.set_suspends();
    let scope = b.param("scope", TypeId::FIBER_SCOPE);
    let left = b.var("left", TypeId::INT);
    b.loop_(None, vec![(left, Const::Int(1000).into())], |b| {
        b.assign(
            "d",
            TypeId::ANY,
            Value::Fiber {
                op: emela_core::FiberOp::Deferred,
                args: vec![scope.into()],
                on_fail: None,
            },
        );
        let zero = b.assign(
            "zero",
            TypeId::BOOL,
            Value::Prim {
                op: PrimOp::Eq,
                ty: PrimTy::I32,
                args: vec![left.into(), Const::Int(0).into()],
            },
        );
        b.if_(
            None,
            zero,
            |b| b.term(Term::Return(None)),
            |b| b.term(Term::Yield(None)),
        );
        let less = b.assign(
            "less",
            TypeId::INT,
            Value::Prim {
                op: PrimOp::Sub,
                ty: PrimTy::I32,
                args: vec![left.into(), Const::Int(1).into()],
            },
        );
        b.term(Term::Recur(vec![less.into()]));
    });
    b.term(Term::Return(None));
    let body = flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "work");
    b.set_exported().set_suspends();
    let body = b.assign("body", ft, Value::FnRef(body));
    b.let_(
        None,
        Value::Fiber {
            op: emela_core::FiberOp::Scope { fails: false },
            args: vec![body.into()],
            on_fail: None,
        },
    );
    b.term(Term::Return(None));
    flow.add(b.finish());
    compile_and_run(
        compile_flow(
            &flow,
            &options().with_deferred_capacity(1).with_heap_bytes(2048),
        )
        .unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    EmelaFiber f = emela_export_work_1_start();
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn complete_stores_once_and_scope_end_breaks_cycles_through_captured_deferreds() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}
pub fn once(n: Int) -> Bool use Async {
  Fiber.scope(fn() {
    d: Deferred[Int] = (use Scope).deferred()
    first = Deferred.complete(d, n)
    second = Deferred.complete(d, n + 1)
    first && !second
  })
}
pub fn cycle() -> Bool use Async {
  Fiber.scope(fn() {
    d: Deferred[fn() -> Bool] = (use Scope).deferred()
    callback = fn() { Deferred.complete(d, fn() { False }) }
    Deferred.complete(d, callback)
  })
}
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    compile_and_run(
        compile(&core, &options().with_deferred_capacity(1)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t result = 0;
    EmelaFiber f = emela_export_once_0_start(i, &result);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == 1); emela_fiber_release(f);
    f = emela_export_cycle_1_start(&result);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == 1); emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn leaked_complete_callback_checks_scope_before_recycled_slot() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}
pub fn leak() -> (fn() -> Bool) use Async {
  Fiber.scope(fn() {
    d: Deferred[Int] = (use Scope).deferred()
    fn() { Deferred.complete(d, 42) }
  })
}
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
        compile(&core, &options().with_deferred_capacity(1)).unwrap(),
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static int expected;
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{
  assert(expected && k == EMELA_DEFECT_PANIC && n == 44);
  assert(memcmp(m, "deferred was completed after its scope ended", 44) == 0); exit(0);
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
    emela_fiber_release(f); emela_embedded_release_function_{ty}(&next);
  }}
  expected = 1; (void)old.call(old.env); abort();
}}
"#
        ),
    );
}

#[test]
fn poll_observes_empty_and_first_value_for_scalar_and_unit_without_yielding() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}
@external(c, "mark")
fn mark(n: Int) -> () use {}
pub fn poll(n: Int) -> Int use Async {
  Fiber.scope(fn() {
    Fiber.fork(fn() { mark(99) })
    d: Deferred[Int] = (use Scope).deferred()
    empty = Deferred.poll(d) match {
      None -> True
      Some(_) -> False
    }
    first = Deferred.complete(d, n)
    again = Deferred.complete(d, n + 1)
    value = Deferred.poll(d) match {
      Some(x) -> x
      None -> 0
    }
    u: Deferred[()] = (use Scope).deferred()
    Deferred.complete(u, ())
    unit = Deferred.poll(u) match {
      Some(()) -> True
      None -> False
    }
    mark(1)
    if empty && first && !again && unit { value } else { 0 }
  })
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
static int marks;
void mark(int32_t n) { assert(n == 1); marks++; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t result = -1;
    EmelaFiber f = emela_export_poll_0_start(i, &result);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == i && marks == i + 1); emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn complete_copies_borrowed_string_and_poll_retains_it_past_scope_end() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}
fn save(d: Deferred[String], n: Int) -> Bool {
  text = Int.to_string(n)
  first = Deferred.complete(d, text)
  again = Deferred.complete(d, text)
  first && !again
}
fn peek(d: Deferred[String]) -> String {
  Deferred.poll(d) match {
    Some(s) -> s
    None -> ""
  }
}
pub fn text(n: Int) -> String use Async {
  Fiber.scope(fn() {
    d: Deferred[String] = (use Scope).deferred()
    save(d, n)
    Int.to_string(n + 1)
    peek(d)
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
            &options().with_string_capacity(16).with_deferred_capacity(1),
        )
        .unwrap(),
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
    EmelaString value = {0};
    EmelaFiber f = emela_export_text_0_start(42, i % 2 ? &value : NULL);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
    if (i % 2) { assert(value.owner && value.length == 2 && memcmp(value.data, "42", 2) == 0); emela_embedded_release_string(&value); }
  }
}
"#,
    );
}

#[test]
fn await_resumes_all_waiters_in_fifo_order_after_completer_finishes() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}
@external(c, "mark")
fn mark(n: Int) -> () use {}
pub fn work() -> Int use Async {
  Fiber.scope(fn() {
    d: Deferred[Int] = (use Scope).deferred()
    a: Deferred[()] = (use Scope).deferred()
    b: Deferred[()] = (use Scope).deferred()
    Fiber.fork(fn() {
      x = Deferred.await(d)
      mark(x + 1)
      Deferred.complete(a, ())
    })
    Fiber.fork(fn() {
      x = Deferred.await(d)
      mark(x + 2)
      Deferred.complete(b, ())
    })
    Fiber.fork(fn() {
      mark(30)
      Deferred.complete(d, 10)
      mark(31)
    })
    mark(0)
    Deferred.await(a)
    Deferred.await(b)
    mark(40)
    Deferred.await(d)
  })
}
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    compile_and_run(
        compile(&core, &options().with_deferred_capacity(3)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
static int count;
void mark(int32_t n) { static const int expected[] = {0,30,31,11,12,40}; assert(count < 6 && n == expected[count++]); }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t result = -1; count = 0;
    EmelaFiber f = emela_export_work_0_start(&result);
    assert(emela_scheduler_run(0) == 0 && count == 0 && result == -1);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == 10 && count == 6); emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn await_preserves_string_and_list_results_past_frame_and_scope_reuse() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}
pub fn text(n: Int) -> String use Async {
  Fiber.scope(fn() {
    d: Deferred[String] = (use Scope).deferred()
    Fiber.fork(fn() { Deferred.complete(d, Int.to_string(n)) })
    Deferred.await(d)
  })
}
pub fn list(n: Int) -> List[Int] use Async {
  Fiber.scope(fn() {
    d: Deferred[List[Int]] = (use Scope).deferred()
    Fiber.fork(fn() { Deferred.complete(d, [n, n + 1]) })
    Deferred.await(d)
  })
}
pub fn length(xs: List[Int]) -> Int { List.length(xs) }
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    compile_and_run(
        compile(&core, &options().with_deferred_capacity(1)).unwrap(),
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
    EmelaFiber f = emela_export_text_0_start(42, i % 2 ? &text : NULL);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
    f = emela_export_list_1_start(i, i % 2 ? &list : NULL);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
    if (i % 2) {
      assert(text.owner && text.length == 2 && memcmp(text.data, "42", 2) == 0);
      assert(emela_export_length_2(list) == 2);
      emela_embedded_release_string(&text); emela_embedded_release(list);
    }
  }
}
"#,
    );
}

#[test]
fn nested_scope_cancellation_unlinks_middle_tail_and_head_waiters_and_ready_deliveries() {
    let source = r#"
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
      Fiber.scope(fn() {
        a: Deferred[()] = (use Scope).deferred()
        b: Deferred[()] = (use Scope).deferred()
        Fiber.fork(fn() {
          Deferred.await(a)
          Deferred.await(d)
          mark(99)
        })
        Fiber.fork(fn() {
          Deferred.await(b)
          Deferred.await(d)
          mark(99)
        })
        Fiber.fork(fn() {
          Deferred.complete(a, ())
          Deferred.complete(b, ())
          Deferred.await(d)
          mark(99)
        })
        (use Wait).get(1)
        if early { Deferred.complete(d, 42) } else { False }
      })
      Deferred.complete(d, 42)
      Deferred.await(d)
    })
  }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    compile_and_run(
        compile(
            &core,
            &options()
                .with_deferred_capacity(3)
                .with_frame_capacity(40)
                .with_task_capacity(1)
                .with_event_capacity(1)
                .with_task_adapter("host_wait", "board_wait_start"),
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
fn deferred_round_trips_int64_and_float_bits_through_await_and_poll() {
    let source = r#"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}
pub fn wide(n: Int64) -> Int64 use Async {
  Fiber.scope(fn() {
    d: Deferred[Int64] = (use Scope).deferred()
    Fiber.fork(fn() { Deferred.complete(d, n) })
    first = Deferred.await(d)
    Deferred.poll(d) match {
      Some(x) -> x
      None -> first
    }
  })
}
pub fn real(n: Float) -> Float use Async {
  Fiber.scope(fn() {
    d: Deferred[Float] = (use Scope).deferred()
    Fiber.fork(fn() { Deferred.complete(d, n) })
    first = Deferred.await(d)
    Deferred.poll(d) match {
      Some(x) -> x
      None -> first
    }
  })
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    compile_and_run(
        compile(&core, &options().with_deferred_capacity(1)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include <math.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int64_t wide = 0; double real = 1, input = i % 2 ? -0.0 : 1.2345678901234567;
    EmelaFiber f = emela_export_wide_0_start(INT64_MIN, &wide);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(wide == INT64_MIN); emela_fiber_release(f);
    f = emela_export_real_1_start(input, &real);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(memcmp(&real, &input, sizeof(double)) == 0); emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn deferred_wait_is_a_real_resume_boundary_and_root_end_reclaims_pending_operation() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}
@external(c, "mark")
fn mark(n: Int) -> () use {}
effect Gate { suspend fn go(d: Deferred[Int]) -> Int }
handler Gated: Gate {
  suspend fn go(d, resume) {
    mark(1)
    resume(7)
    Deferred.await(d)
    mark(99)
  }
}
pub fn boundary() -> Int use Async {
  Fiber.scope(fn() {
    d: Deferred[Int] = (use Scope).deferred()
    Fiber.fork(fn() { mark(2) })
    result = with Gated { (use Gate).go(d) }
    mark(3)
    result
  })
}
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    compile_and_run(
        compile(&core, &options().with_deferred_capacity(1)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
static int marks;
void mark(int32_t n) { assert(n == ++marks); }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t result = -1; marks = 0;
    EmelaFiber f = emela_export_boundary_0_start(&result);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == 7 && marks == 3); emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn deferred_pool_exhaustion_uses_the_defined_defect() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}
pub fn full() -> Bool use Async {
  Fiber.scope(fn() {
    a: Deferred[Int] = (use Scope).deferred()
    b: Deferred[Int] = (use Scope).deferred()
    Deferred.complete(a, 1) && Deferred.complete(b, 2)
  })
}
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    compile_and_run(
        compile(&core, &options().with_deferred_capacity(1)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
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
fn ended_deferred_poll_and_await_reject_leaked_callbacks_after_slot_reuse() {
    for awaiting in [false, true] {
        let operation = if awaiting {
            "Deferred.await(d)"
        } else {
            "Deferred.poll(d) match { Some(_) -> True\nNone -> False }"
        };
        let ret = if awaiting { "Int use Async" } else { "Bool" };
        let invoke = if awaiting {
            "pub fn invoke(f: fn() -> Int fails {} use Async) -> Int use Async { f() }"
        } else {
            ""
        };
        let source = format!(
            r#"
import Std.Fiber.{{Fiber, Scope}}
import Std.Deferred.{{Deferred}}
pub fn leak() -> (fn() -> {ret}) use Async {{
  Fiber.scope(fn() {{
    d: Deferred[Int] = (use Scope).deferred()
    fn() {{ {operation} }}
  }})
}}
{invoke}
"#
        );
        let core = support::try_lower(&[("main.emel", &source)], Roots::Exported).unwrap();
        let flow = emela_codegen_emb::flow_from_core(&core).unwrap();
        let ty = flow
            .functions
            .iter()
            .find(|(_, f)| f.name == "leak")
            .unwrap()
            .1
            .ret
            .index();
        let message = if awaiting {
            "deferred was awaited after its scope ended"
        } else {
            "deferred was polled after its scope ended"
        };
        let call = if awaiting {
            "int32_t result = 0; f = emela_export_invoke_1_start(old, &result); while (!emela_fiber_done(f)) emela_scheduler_run(1);"
        } else {
            "(void)old.call(old.env);"
        };
        compile_and_run(
            compile(&core, &options().with_deferred_capacity(1)).unwrap(),
            &format!(
                r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static int expected;
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ assert(expected && k == EMELA_DEFECT_PANIC && n == {len}); assert(memcmp(m, "{message}", n) == 0); exit(0); }}
int main(void) {{
  emela_function_type_{ty} old = {{0}};
  EmelaFiber f = emela_export_leak_0_start(&old);
  while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
  emela_fiber_release(f);
  for (int i = 0; i < 1000; i++) {{
    emela_function_type_{ty} next = {{0}};
    f = emela_export_leak_0_start(&next);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f); emela_embedded_release_function_{ty}(&next);
  }}
  expected = 1; {call} abort();
}}
"#,
                len = message.len()
            ),
        );
    }
}

#[test]
fn deferred_transfers_tuple_values_and_boolean_flags() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}
pub fn pair(n: Int) -> Int use Async {
  Fiber.scope(fn() {
    d: Deferred[(Int, Int)] = (use Scope).deferred()
    Fiber.fork(fn() { Deferred.complete(d, (n, n + 1)) })
    (a, b) = Deferred.await(d)
    a + b
  })
}
pub fn flag(n: Bool) -> Bool use Async {
  Fiber.scope(fn() {
    d: Deferred[Bool] = (use Scope).deferred()
    Fiber.fork(fn() { Deferred.complete(d, n) })
    Deferred.await(d)
  })
}
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    compile_and_run(
        compile(&core, &options().with_deferred_capacity(1)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t result = 0;
    EmelaFiber f = emela_export_pair_0_start(i, &result);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == 2 * i + 1); emela_fiber_release(f);
    f = emela_export_flag_1_start(i % 2, &result);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == i % 2); emela_fiber_release(f);
  }
}
"#,
    );
}
