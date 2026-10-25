use super::*;

#[test]
fn await_returns_success_and_can_mix_with_repeated_join_without_yielding() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Exit, Success, Failure}
@external(c, "mark")
fn mark(n: Int) -> () use {}
pub fn work(n: Int) -> Int use Async {
  Fiber.scope(fn() {
    child = Fiber.fork(fn() { mark(2)
      n })
    mark(1)
    first = Fiber.await(child) match { Success(v) -> v
      Failure(_) -> -1 }
    Fiber.fork(fn() { mark(99) })
    Fiber.await(child)
    next = Fiber.await(child) match { Success(v) -> v
      Failure(_) -> -2 }
    mark(3)
    first + next + Fiber.join(child)
  })
}
pub fn unit() -> Bool use Async {
  Fiber.scope(fn() {
    child = Fiber.fork(fn() { () })
    Fiber.await(child) match { Success(_) -> True
      Failure(_) -> False }
  })
}
pub fn discard() -> () use Async {
  Fiber.scope(fn() {
    child = Fiber.fork(fn() { mark(1) })
    Fiber.await(child)
    mark(2)
  })
}
"#,
        )],
        Roots::Public,
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
    int32_t result = -1; order = 0;
    EmelaFiber f = emela_export_work_0_start(i, &result);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == 3 * i && order == 3); emela_fiber_release(f);
    result = -1; f = emela_export_unit_1_start(&result);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == 1); emela_fiber_release(f);
    order = 0; f = emela_export_discard_2_start();
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(order == 2); emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn await_keeps_owned_list_results_after_the_scope_ends() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Success, Failure}
pub fn list(n: Int) -> List[Int] use Async {
  Fiber.scope(fn() {
    child = Fiber.fork(fn() { [n, n + 1] })
    first = Fiber.await(child) match { Success(v) -> v
      Failure(_) -> [] }
    Fiber.await(child)
    second = Fiber.await(child) match { Success(v) -> v
      Failure(_) -> [] }
    List.append(first, second)
  })
}
pub fn sum(xs: List[Int]) -> Int { List.fold(xs, 0, fn(a, b) { a + b }) }
"#,
        )],
        Roots::Public,
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
    void *value = NULL;
    EmelaFiber f = emela_export_list_0_start(i, i % 2 ? &value : NULL);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
    if (i % 2) {
      assert(emela_export_sum_1(value) == 4 * i + 2);
      emela_embedded_release(value);
    }
  }
}
"#,
    );
}

#[test]
fn await_and_join_share_fifo_waiters_and_transfer_each_result() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Success, Failure}
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
        v = Fiber.await(child) match { Success(v) -> v
          Failure(_) -> -1 }
        mark(5)
        v })
      b = Fiber.fork(fn() { mark(2)
        v = Fiber.join(child)
        mark(6)
        v })
      c = Fiber.fork(fn() { mark(3)
        Fiber.await(child)
        mark(7) })
      result = Fiber.join(a) + Fiber.join(b)
      Fiber.join(c)
      result
    })
  }
}
"#,
        )],
        Roots::Public,
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
    assert(result == 84 && order == 7); emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn leaked_await_callback_checks_the_ended_scope_before_using_the_child_result() {
    let source = r#"
import Std.Fiber.{Fiber, Success, Failure}
pub fn leak() -> (fn() -> Int use Async) use Async {
  Fiber.scope(fn() {
    child = Fiber.fork(fn() { 42 })
    Fiber.join(child)
    fn() { Fiber.await(child) match { Success(v) -> v
      Failure(_) -> -1 } }
  })
}
pub fn invoke(f: fn() -> Int fails {} use Async) -> Int use Async { f() }
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Public).unwrap();
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
  assert(expected && k == EMELA_DEFECT_PANIC && n == 38);
  assert(memcmp(m, "fiber was awaited after its scope ended", n) == 0); exit(0);
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
  expected = 1; int32_t result = 0; f = emela_export_invoke_1_start(old, &result);
  while (!emela_fiber_done(f)) emela_scheduler_run(1);
  abort();
}}
"#
        ),
    );
}

#[test]
fn child_wait_is_a_resume_boundary_and_scope_end_reclaims_the_operation_tail() {
    for operation in ["join", "await"] {
        let source = format!(
            r#"
import Std.Fiber.{{Fiber, Scope}}
import Std.Deferred.{{Deferred}}
@external(c, "mark")
fn mark(n: Int) -> () use {{}}
effect Gate {{ suspend fn go(child: Fiber[Int]) -> Int }}
handler Gated: Gate {{
  suspend fn go(child, resume) {{
    mark(1)
    resume(7)
    Fiber.{operation}(child)
    mark(99)
  }}
}}
pub fn boundary() -> Int use Async {{
  Fiber.scope(fn() {{
    d: Deferred[Int] = (use Scope).deferred()
    child = Fiber.fork(fn() {{ mark(2)
      Deferred.await(d) }})
    result = with Gated {{ (use Gate).go(child) }}
    mark(3)
    result
  }})
}}
"#
        );
        let core = support::try_lower(&[("main.emel", &source)], Roots::Public).unwrap();
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
}

#[test]
fn scope_end_breaks_cycles_when_a_child_result_closure_captures_its_own_handle() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}
pub fn cycle() -> Bool use Async {
  Fiber.scope(fn() {
    d: Deferred[Fiber[fn() -> Bool use Async]] = (use Scope).deferred()
    child = Fiber.fork(fn() {
      me = Deferred.await(d)
      fn() { Fiber.join(me)
        True }
    })
    Deferred.complete(d, child)
    callback = Fiber.join(child)
    callback()
  })
}
"#,
        )],
        Roots::Public,
    )
    .unwrap();
    compile_and_run(
        compile(
            &core,
            &options().with_heap_bytes(4096).with_deferred_capacity(1),
        )
        .unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t result = -1;
    EmelaFiber f = emela_export_cycle_0_start(&result);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == 1); emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn closing_a_scope_drops_ready_join_and_await_frames_before_result_delivery() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}
@external(c, "mark")
fn mark(n: Int) -> () use {}
effect Gate { suspend fn go(start: Deferred[fn(Int) -> ()], stop: Deferred[()]) -> Int }
handler Gated: Gate {
  suspend fn go(start, stop, resume) {
    Deferred.complete(start, resume)
    Deferred.await(stop)
  }
}
pub fn ready() -> Int use Async {
  Fiber.scope(fn() {
    start: Deferred[fn(Int) -> ()] = (use Scope).deferred()
    stop: Deferred[()] = (use Scope).deferred()
    target = Fiber.fork(fn() {
      resume = Deferred.await(start)
      resume(7)
      42
    })
    result = Fiber.scope(fn() {
      registered: Deferred[()] = (use Scope).deferred()
      Fiber.fork(fn() { Fiber.join(target)
        mark(99) })
      Fiber.fork(fn() { Fiber.await(target)
        mark(99) })
      Fiber.fork(fn() { Deferred.complete(registered, ()) })
      Deferred.await(registered)
      with Gated { (use Gate).go(start, stop) }
    })
    result + Fiber.join(target)
  })
}
"#,
        )],
        Roots::Public,
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
void mark(int32_t n) { (void)n; abort(); }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t result = -1;
    EmelaFiber f = emela_export_ready_0_start(&result);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == 49); emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn await_preserves_int64_float_bits_and_bool_payloads() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Success, Failure}
pub fn wide(n: Int64) -> Int64 use Async {
  Fiber.scope(fn() { child = Fiber.fork(fn() { n })
    Fiber.await(child) match { Success(v) -> v
      Failure(_) -> n } })
}
pub fn real(n: Float) -> Float use Async {
  Fiber.scope(fn() { child = Fiber.fork(fn() { n })
    Fiber.await(child) match { Success(v) -> v
      Failure(_) -> 0.0 } })
}
pub fn flag(n: Bool) -> Bool use Async {
  Fiber.scope(fn() { child = Fiber.fork(fn() { n })
    Fiber.await(child) match { Success(v) -> v
      Failure(_) -> False } })
}
"#,
        )],
        Roots::Public,
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

#[test]
fn await_with_a_string_exit_keeps_owned_backing_after_scope_and_frame_reuse() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Success, Failure}
pub fn text(n: Int) -> String use Async {
  Fiber.scope(fn() {
    child = Fiber.fork(fn() { Int.to_string(n) })
    Fiber.await(child) match { Success(v) -> v
      Failure(_) -> "failed" }
  })
}
"#,
        )],
        Roots::Public,
    )
    .unwrap();
    let mut output = compile(&core, &options().with_heap_bytes(4096)).unwrap();
    notes::add_heap_check(&mut output);
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    EmelaString result = {0};
    EmelaFiber f = emela_export_text_0_start(INT32_MIN, i % 3 ? &result : NULL);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
    if (i % 3) {
      assert(result.owner && result.length == 11 && memcmp(result.data, "-2147483648", 11) == 0);
      emela_embedded_release_string(&result);
    }
    assert(test_heap_empty());
  }
}
"#,
    );
}

#[test]
fn returning_await_exit_through_scope_owns_the_exit_after_scope_end() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Exit}
pub fn terminal(n: Int) -> Exit[Int, {}] use Async {
  Fiber.scope(fn() { child = Fiber.fork(fn() { n })
    Fiber.await(child) })
}
"#,
        )],
        Roots::Public,
    )
    .unwrap();
    let mut output = compile(&core, &options()).unwrap();
    notes::add_heap_check(&mut output);
    let header = &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.h")
        .unwrap()
        .contents;
    let ty: usize = header
        .lines()
        .find(|line| line.starts_with("EmelaFiber emela_export_terminal_0_start("))
        .unwrap()
        .split("emela_type_")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let release = aggregate::release_helper(&output, ty);
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ (void)k; (void)m; (void)n; abort(); }}
int main(void) {{
  for (int i = 0; i < 1000; ++i) {{
    emela_type_{ty} value = {{0}};
    EmelaFiber f = emela_export_terminal_0_start(i, i % 3 ? &value : NULL);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
    if (i % 3) {{ assert(value.tag == 0 && value.payload.v0.f0 == i); {release}(&value); }}
    assert(test_heap_empty());
  }}
}}
"#
        ),
    );
}

#[test]
fn a_direct_suspend_call_can_return_an_inline_exit_into_its_caller_frame() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Exit, Success, Failure}
fn terminal(child: Fiber[Int]) -> Exit[Int, {}] use Async { Fiber.await(child) }
pub fn work(n: Int) -> Int use Async {
  Fiber.scope(fn() {
    child = Fiber.fork(fn() { n })
    terminal(child) match { Success(v) -> v
      Failure(_) -> -1 }
  })
}
"#,
        )],
        Roots::Public,
    )
    .unwrap();
    let flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    let id = flow
        .functions
        .iter()
        .find(|(_, f)| f.name == "work")
        .unwrap()
        .0;
    let id = u32::from(id.into_raw());
    compile_and_run(
        compile(&core, &options().with_heap_bytes(4096)).unwrap(),
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ (void)k; (void)m; (void)n; abort(); }}
int main(void) {{
  for (int i = 0; i < 1000; i++) {{
    int32_t value = -1;
    EmelaFiber f = emela_export_work_{id}_start(i, &value);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(value == i); emela_fiber_release(f);
    f = emela_export_work_{id}_start(i, NULL);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
  }}
}}
"#
        ),
    );
}
