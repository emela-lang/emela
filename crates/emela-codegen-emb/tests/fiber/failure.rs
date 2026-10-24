use super::*;

#[test]
fn suspend_failures_cross_calls_and_preserve_cleanup_before_escape() {
    let source = r#"
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
@external(c, "mark")
fn mark(n: Int) -> () use {}
error Boom(n: Int) derive Show
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting: Wait {
  suspend fn get(n, resume) { resume(wait(n)) }
  release { mark(2) }
}
fn failure(n: Int) -> Int fails Boom { fail Boom(n: n) }
fn work(n: Int) -> Int fails Boom use Async {
  with Waiting {
    x = (use Wait).get(n)
    mark(1)
    failure(x)
  }
}
pub fn caught(n: Int) -> Int use Async {
  work(n) escape { Boom(n) -> {
    mark(3)
    n
  } }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    compile_and_run(
        compile(&core, &options().with_heap_bytes(4096)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask task;
static int order;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t, int32_t n) { assert(n == 41); task = t; }
void mark(int32_t n) { assert(n == ++order); }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k,n,m); abort(); }
int main(void) {
  for (int i=0; i<1000; i++) {
    int32_t result=-1; order=0;
    EmelaFiber f=emela_export_caught_0_start(41, &result);
    while (emela_scheduler_run(1)) {}
    assert(!emela_fiber_done(f) && order==0);
    assert(emela_task_complete_int(task, 42));
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result==42 && order==3);
    emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn failing_suspend_closure_keeps_error_through_suspending_release() {
    let source = r#"
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
@external(c, "mark")
fn mark(n: Int) -> () use {}
error Boom(n: Int) derive Show
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting: Wait { suspend fn get(n, resume) { resume(wait(n)) } }
effect Resource { fn read() -> Int }
handler Res: Resource {
  fn read() { 0 }
  release { mark(2)
    (use Wait).get(2)
    mark(3)
  }
}
fn invoke(f: fn() -> Int fails Boom use Wait) -> Int fails Boom use Wait { f() }
pub fn caught(n: Int) -> Int use Async {
  with Waiting {
    invoke(fn() {
      with Res {
        (use Wait).get(1)
        mark(1)
        fail Boom(n: n)
      }
    }) escape { Boom(n) -> { mark(4)
      n
    } }
  }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    compile_and_run(
        compile(&core, &options().with_heap_bytes(4096)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask task;
static int order, waits;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t, int32_t n) { assert(n==++waits); task=t; }
void mark(int32_t n) { assert(n==++order); }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m); abort(); }
int main(void) {
 for(int i=0;i<1000;i++) {
  order=waits=0; int32_t result=-1;
  EmelaFiber f=emela_export_caught_0_start(42,&result);
  while(emela_scheduler_run(1)) {}
  assert(waits==1 && order==0);
  assert(emela_task_complete_int(task,0));
  while(emela_scheduler_run(1)) {}
  assert(waits==2 && order==2 && result==-1 && !emela_fiber_done(f));
  assert(emela_task_complete_int(task,0));
  while(!emela_fiber_done(f)) assert(emela_scheduler_run(1));
  assert(order==4 && result==42); emela_fiber_release(f);
 }
}
"#,
    );
}

#[test]
fn uncaught_suspend_failure_stops_at_public_boundary_after_release() {
    let source = r#"
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
@external(c, "mark")
fn mark(n: Int) -> () use {}
error Boom
effect Wait { suspend fn get() -> Int }
handler Waiting: Wait {
 suspend fn get(resume) { resume(wait(1)) }
 release { mark(1) }
}
pub fn failure() -> Int fails Boom use Async {
 with Waiting { (use Wait).get()
  fail Boom
 }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    compile_and_run(
        compile(&core, &options()).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask task;
static int released;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t,int32_t n) { (void)n;task=t; }
void mark(int32_t n) { assert(n==1); released++; }
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) {
 assert(k==EMELA_DEFECT_UNHANDLED_ERROR && n==15 && memcmp(m,"unhandled error",15)==0 && released==1); exit(0);
}
int main(void) {
 EmelaFiber f=emela_export_failure_0_start(NULL);
 while(emela_scheduler_run(1)) {}
 assert(emela_task_complete_int(task,0));
 while(!emela_fiber_done(f)) assert(emela_scheduler_run(1));
 abort();
}
"#,
    );
}

#[test]
fn child_failure_closes_scope_and_waits_for_sibling_release_before_escape() {
    let source = r#"
import Std.Fiber.{Fiber}
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
@external(c, "mark")
fn mark(n: Int) -> () use {}
error Boom(n: Int) derive Show
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting: Wait { suspend fn get(n,resume) { resume(wait(n)) } }
effect Resource { fn read() -> Int }
handler Res: Resource {
 fn read() { 0 }
 release { mark(1)
  (use Wait).get(3)
  mark(2)
 }
}
pub fn caught(mode: Int) -> Int use Async {
 with Waiting {
  Fiber.scope(fn() {
   Fiber.fork(fn() { with Res { (use Wait).get(2) } })
   Fiber.fork(fn() { if mode == 0 { fail Boom(n: 42) } else { 0 } })
   (use Wait).get(1)
   if mode != 0 { fail Boom(n: 42) }
   mark(99)
   0
  }) escape { Boom(n) -> { mark(3)
   n
  } }
 }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    compile_and_run(
        compile(
            &core,
            &options()
                .with_heap_bytes(8192)
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
static int marks,waits;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t,int32_t n) { assert(n==++waits);tasks[n-1]=t; }
void mark(int32_t n) { assert(n==++marks); }
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m);abort(); }
int main(void) {
 for(int i=0;i<1000;i++) {
  marks=waits=0;int32_t result=-1;
  EmelaFiber f=emela_export_caught_0_start(i%2, &result);
  while(emela_scheduler_run(1)) {}
  if(i%2) {
   assert(waits==2 && marks==0 && !emela_fiber_done(f));
   assert(emela_task_complete_int(tasks[0],0));
   while(emela_scheduler_run(1)) {}
  }
  assert(waits==3 && marks==1 && result==-1 && !emela_fiber_done(f));
  assert(!emela_task_complete_int(tasks[0],99));
  assert(!emela_task_complete_int(tasks[1],99));
  assert(emela_task_complete_int(tasks[2],0));
  while(!emela_fiber_done(f)) assert(emela_scheduler_run(1));
  assert(result==42 && marks==3);emela_fiber_release(f);
 }
}
"#,
    );
}

#[test]
fn suspend_operation_failure_before_resume_reaches_its_call_edge() {
    let source = r#"
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
error Boom(n: Int) derive Show
effect Wait { suspend fn get(n: Int) -> Int fails Boom }
handler Waiting: Wait {
 suspend fn get(n,resume) {
  x = wait(n)
  if n == 0 { resume(x) } else { fail Boom(n: x) }
 }
}
pub fn caught(n: Int) -> Int use Async {
 with Waiting { (use Wait).get(n) escape { Boom(n) -> n } }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    compile_and_run(
        compile(&core, &options().with_heap_bytes(2048)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask task;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t,int32_t n) { (void)n;task=t; }
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m);abort(); }
int main(void) {
 for(int i=0;i<1000;i++) {
  int32_t result=-1;EmelaFiber f=emela_export_caught_0_start(i%2,&result);
  while(emela_scheduler_run(1)) {}
  assert(emela_task_complete_int(task,42));
  while(!emela_fiber_done(f)) assert(emela_scheduler_run(1));
  assert(result==42);emela_fiber_release(f);
 }
}
"#,
    );
}

#[test]
fn operation_failure_after_resume_stops_without_writing_a_stale_error_pointer() {
    let source = r#"
error Boom
effect Tick { suspend fn tick() -> Int fails Boom }
handler Ticking: Tick {
 suspend fn tick(resume) { resume(42)
  fail Boom
 }
}
pub fn work() -> Int use Async {
 with Ticking { (use Tick).tick() escape { Boom -> 0 } }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    compile_and_run(
        compile(&core, &options()).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) {
 const char *expected="operation `tick` of handler `Ticking` failed after resume: Boom";
 assert(k==EMELA_DEFECT_PANIC && n==strlen(expected) && memcmp(m,expected,n)==0); exit(0);
}
int main(void) {
 EmelaFiber f=emela_export_work_0_start(NULL);
 while(!emela_fiber_done(f)) assert(emela_scheduler_run(1));
 abort();
}
"#,
    );
}

#[test]
fn masked_cleanup_can_await_the_failed_child_before_scope_error_propagates() {
    let source = r#"
import Std.Fiber.{Fiber, Success, Failure}
import Std.Cause.{Cause, Fail, Die, Interrupt}
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
@external(c, "mark")
fn mark(n: Int) -> () use {}
error Boom(n: Int) derive Show
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting: Wait { suspend fn get(n,resume) { resume(wait(n)) } }
fn cleanup(child: Fiber[Int], joined: Bool) -> () use Async {
 if joined { Fiber.join(child)
  mark(99)
 } else { Fiber.await(child) match { Success(_) -> mark(99)
  Failure(c) -> Cause.kind(c) match { Fail -> mark(1)
   Die -> mark(98)
   Interrupt -> mark(97)
  }
 }
}
}
fn protected(child: Fiber[Int], joined: Bool) -> Int use {Async, Wait} {
 (use Wait).get(2)
 cleanup(child, joined)
 0
}
pub fn work(joined: Bool) -> Int use Async {
 with Waiting {
  Fiber.scope(fn() {
   child = Fiber.fork(fn() { (use Wait).get(1)
    fail Boom(n: 42)
   })
   protected(child, joined)
  }) escape { Boom(n) -> { mark(2)
   n
  } }
 }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    let mut flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    super::interrupt::protect_cleanup_call(&mut flow);
    compile_and_run(
        compile_flow(&flow, &options()).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask tasks[2];static int waits, marks, joined;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t,int32_t n) { assert(n>=1 && n<=2);tasks[n-1]=t;waits++; }
void mark(int32_t n) { if(n==1) { assert(!joined && marks++==0); } else { assert(n==2 && marks++==!joined); } }
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m);abort(); }
int main(void) {
 for(int i=0;i<1000;i++) {
  waits=marks=0;joined=i%2;int32_t result=-1;EmelaFiber f=emela_export_work_0_start(joined,&result);
  while(emela_scheduler_run(1)) {}assert(waits==2);
  assert(emela_task_complete_int(tasks[0],0));
  while(!emela_fiber_done(f)) assert(emela_scheduler_run(1));
  assert(result==42 && marks==2-joined && !emela_task_complete_int(tasks[1],0));emela_fiber_release(f);
 }
}
"#,
    );
}

#[test]
fn caught_failure_inside_suspending_release_does_not_replace_the_original_error() {
    let mut core = emela_core::Module::new();
    let error = core.add_enum(EnumDef {
        name: "Boom".into(),
        params: 0,
        variants: vec![VariantDef::positional("Boom", vec![TypeId::INT])],
        error: true,
        external: false,
    });
    let error_ty = core.enum_type(error, vec![]);
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "failure");
    b.set_fails().set_ret(TypeId::INT);
    let n = b.param("n", TypeId::INT);
    let value = b.assign(
        "error",
        error_ty,
        Value::Make {
            ctor: CtorRef {
                enum_id: error,
                variant: 0,
            },
            args: vec![n.into()],
        },
    );
    b.term(Term::Fail(value.into()));
    let failure = flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "wait");
    b.set_suspends();
    let wait = flow.add(b.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("host_wait".into()),
        }],
    }));
    let mut b = FnBuilder::new(&flow.core, "mark");
    b.param("n", TypeId::INT);
    let mark = flow.add(b.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("mark".into()),
        }],
    }));
    let mut b = FnBuilder::new(&flow.core, "work");
    b.set_exported().set_suspends().set_ret(TypeId::INT);
    let caught = b.var("caught", TypeId::ANY);
    b.block(Some(caught), "caught", |b, label| {
        b.protect(
            None,
            |b| {
                b.let_(
                    None,
                    Value::Call {
                        callee: Callee::Direct(failure),
                        args: vec![Const::Int(42).into()],
                        suspends: false,
                        on_fail: Some(OnFail::Break(label)),
                    },
                );
                b.term(Term::Unreachable);
            },
            |b| {
                let nested = b.var("nested", TypeId::ANY);
                b.block(Some(nested), "release_error", |b, label| {
                    b.let_(
                        None,
                        Value::Call {
                            callee: Callee::Direct(failure),
                            args: vec![Const::Int(7).into()],
                            suspends: false,
                            on_fail: Some(OnFail::Break(label)),
                        },
                    );
                    b.term(Term::Unreachable);
                });
                let typed = b.assign(
                    "typed",
                    error_ty,
                    Value::AsError {
                        enum_id: error,
                        error: nested,
                    },
                );
                let n = b.assign(
                    "n",
                    TypeId::INT,
                    Value::Field {
                        ctor: CtorRef {
                            enum_id: error,
                            variant: 0,
                        },
                        base: typed,
                        index: 0,
                    },
                );
                b.let_(
                    None,
                    Value::Call {
                        callee: Callee::Direct(mark),
                        args: vec![n.into()],
                        suspends: false,
                        on_fail: None,
                    },
                );
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
        b.term(Term::Unreachable);
    });
    let typed = b.assign(
        "typed",
        error_ty,
        Value::AsError {
            enum_id: error,
            error: caught,
        },
    );
    let n = b.assign(
        "n",
        TypeId::INT,
        Value::Field {
            ctor: CtorRef {
                enum_id: error,
                variant: 0,
            },
            base: typed,
            index: 0,
        },
    );
    b.term(Term::Return(Some(n.into())));
    flow.add(b.finish());
    compile_and_run(
        compile_flow(&flow, &options().with_heap_bytes(0)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask task;static int marks;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t) { task=t; }
void mark(int32_t n) { assert(n==7);marks++; }
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m);abort(); }
int main(void) {
 for(int i=0;i<1000;i++) {
  marks=0;int32_t result=-1;EmelaFiber f=emela_export_work_3_start(&result);
  while(emela_scheduler_run(1)) {}assert(marks==1 && result==-1);
  assert(emela_task_complete_unit(task));
  while(!emela_fiber_done(f)) assert(emela_scheduler_run(1));
  assert(result==42);emela_fiber_release(f);
 }
}
"#,
    );
}

#[test]
fn delayed_failure_after_resume_formats_owned_error_after_the_call_frame_returns() {
    let source = r#"
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
error Boom(n: Int) derive Show
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting: Wait { suspend fn get(n,resume) { resume(wait(n)) } }
effect Tick { suspend fn tick() -> Int fails Boom }
handler Ticking: Tick {
 suspend fn tick(resume) { resume(42)
  (use Wait).get(1)
  fail Boom(n: -2147483647 - 1)
 }
}
fn issue() -> Int use { Tick } { (use Tick).tick() escape { Boom(_) -> -1 } }
pub fn work() -> Int use Async {
 with Waiting { with Ticking {
  n = issue()
  (use Wait).get(2)
  n
 } }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    compile_and_run(
        compile(&core, &options()).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask tasks[2]; static int32_t result=-99; static int phase, count;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t,int32_t n) { assert(n==1 || n==2);tasks[n-1]=t;count++; }
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) {
 const char *expected="operation `tick` of handler `Ticking` failed after resume: Boom(n: -2147483648)";
 assert(phase==1 && result==-99 && k==EMELA_DEFECT_PANIC && n==strlen(expected) && memcmp(m,expected,n)==0);exit(0);
}
int main(void) {
 EmelaFiber f=emela_export_work_0_start(&result);
 while(emela_scheduler_run(1)) {} assert(!emela_fiber_done(f) && count==2);
 phase=1;assert(emela_task_complete_int(tasks[0],0));
 while(emela_scheduler_run(1)) {} abort();
}
"#,
    );
}

#[test]
fn a_failing_suspend_entry_without_unwind_features_needs_no_unwind_fields() {
    let mut core = emela_core::Module::new();
    let error = core.add_enum(EnumDef {
        name: "Boom".into(),
        params: 0,
        variants: vec![VariantDef::positional("Boom", vec![TypeId::INT])],
        error: true,
        external: false,
    });
    let ty = core.enum_type(error, vec![]);
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "failure");
    b.set_exported()
        .set_suspends()
        .set_fails()
        .set_ret(TypeId::INT);
    let value = b.assign(
        "error",
        ty,
        Value::Make {
            ctor: CtorRef {
                enum_id: error,
                variant: 0,
            },
            args: vec![Const::Int(42).into()],
        },
    );
    b.term(Term::Fail(value.into()));
    flow.add(b.finish());
    compile_and_run(
        compile_flow(
            &flow,
            &options()
                .with_heap_bytes(0)
                .with_fiber_capacity(1)
                .with_frame_capacity(2)
                .with_frame_bytes(128)
                .with_task_capacity(0)
                .with_event_capacity(0),
        )
        .unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) {
 assert(k==EMELA_DEFECT_UNHANDLED_ERROR && n==15 && memcmp(m,"unhandled error",15)==0);exit(0);
}
int main(void) {
 EmelaFiber f=emela_export_failure_0_start(NULL);
 while(!emela_fiber_done(f)) { assert(emela_scheduler_run(1)); }
 abort();
}
"#,
    );
}

#[test]
fn failing_and_infallible_scopes_share_the_fork_operation_abi() {
    let source = include_str!("../../examples/support/scope_failure_abi.emel");
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    let mut output = compile(
        &core,
        &options().with_heap_bytes(4096).with_frame_capacity(32),
    )
    .unwrap();
    super::notes::add_heap_check(&mut output);
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m);abort(); }
int main(void) {
 for(int i=0;i<1000;i++) {
  for(int which=0;which<3;which++) {
   int32_t result=-1;EmelaFiber f=which==2 ? emela_export_nested_2_start(i,&result):(which ? emela_export_bad_1_start(i,&result):emela_export_good_0_start(i,&result));
   while(!emela_fiber_done(f)) { assert(emela_scheduler_run(1)); }
   assert(result==(which==2 ? 2*i+1:i+!which));emela_fiber_release(f);assert(test_heap_empty());
  }
 }
}
"#,
    );
}
