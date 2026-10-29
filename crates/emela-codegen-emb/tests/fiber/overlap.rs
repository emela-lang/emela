use super::*;
use emela_lir::Stmt;

#[test]
fn nested_scope_failure_survives_external_interruption_before_and_after_failure() {
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
handler Res: Resource { fn read() { 0 }
 release { mark(1)
  (use Wait).get(4)
  mark(2)
 }
}
fn late() -> Int fails Boom use Wait { (use Wait).get(1)
 fail Boom(n: 42)
}
fn nested() -> Int fails Boom use {Wait,Async} {
 with Res { Fiber.scope(fn() { Fiber.scope(fn() {
  Fiber.fork(late)
  (use Wait).get(2)
 }) }) }
}
pub fn work() -> Int use Async {
 with Waiting { Fiber.scope(fn() {
  child = Fiber.fork(nested)
  (use Wait).get(3)
  Fiber.interrupt(child)
  99
 }) escape { Boom(n) -> { mark(3)
  n
 } } }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Public).unwrap();
    let mut flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    let (_, late) = flow
        .functions
        .iter_mut()
        .find(|(_, f)| f.name == "late")
        .unwrap();
    // Model a failure delivered while the child's interruption is held.
    let hold = late.stmts.alloc(Stmt::Let {
        dst: None,
        on_defect: None,
        on_suspend: None,
        value: Value::Runtime {
            op: RuntimeOp::Hold,
            args: vec![],
        },
    });
    late.seqs[late.body.unwrap()].stmts.insert(0, hold);
    let mut output = compile_flow(
        &flow,
        &options()
            .with_heap_bytes(4096)
            .with_fiber_capacity(10)
            .with_frame_capacity(48)
            .with_task_capacity(4)
            .with_event_capacity(4),
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
static EmelaTask tasks[4];static int count, order;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t,int32_t n) { assert(n>=1 && n<=4);tasks[n-1]=t;count++; }
void mark(int32_t n) { assert(n==++order); }
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m);abort(); }
int main(void) {
 for(int i=0;i<1000;i++) {
  count=order=0;int32_t result=-1;EmelaFiber f=emela_export_work_0_start(&result);
  while(emela_scheduler_run(1)) {} assert(count==3);
  int first=i%2 ? 1:3,second=i%2 ? 3:1;
  assert(emela_task_complete_int(tasks[first-1],0));while(emela_scheduler_run(1)) {}
  assert(emela_task_complete_int(tasks[second-1],0));while(emela_scheduler_run(1)) {}
  assert(count==4 && order==1 && !emela_fiber_done(f));
  assert(emela_task_complete_int(tasks[3],0));
  while(!emela_fiber_done(f)) { assert(emela_scheduler_run(1)); }
  assert(result==42 && order==3 && !emela_task_complete_int(tasks[1],0));emela_fiber_release(f);assert(test_heap_empty());
 }
}
"#,
    );
}

#[test]
fn failures_from_cancelled_nested_scopes_become_notes_of_the_outer_first_failure() {
    let source = r#"
import Std.Fiber.{Fiber, Success, Failure}
import Std.Cause.{Cause}
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
error Boom(n: Int) derive Show
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting: Wait { suspend fn get(n,resume) { resume(wait(n)) } }
fn late(n: Int) -> Int fails Boom use Wait { (use Wait).get(n)
 fail Boom(n: n)
}
pub fn work() -> Cause[Boom] use Async {
 with Waiting { Fiber.attempt(fn() { Fiber.scope(fn() {
  Fiber.fork(fn() { Fiber.scope(fn() { Fiber.scope(fn() {
   Fiber.fork(fn() { late(1) })
   (use Wait).get(2)
  }) }) })
  Fiber.fork(fn() { late(3) })
  (use Wait).get(4)
 }) }) match { Success(_) -> panic("unexpected")
  Failure(c) -> c
 } }
}
pub fn show(c: Cause[Boom]) -> String { Cause.to_string(c) }
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Public).unwrap();
    let mut flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    let (_, late) = flow
        .functions
        .iter_mut()
        .find(|(_, f)| f.name == "late")
        .unwrap();
    let hold = late.stmts.alloc(Stmt::Let {
        dst: None,
        on_defect: None,
        on_suspend: None,
        value: Value::Runtime {
            op: RuntimeOp::Hold,
            args: vec![],
        },
    });
    late.seqs[late.body.unwrap()].stmts.insert(0, hold);
    let mut output = compile_flow(
        &flow,
        &options()
            .with_heap_bytes(8192)
            .with_fiber_capacity(10)
            .with_frame_capacity(48)
            .with_task_capacity(4)
            .with_event_capacity(4),
    )
    .unwrap();
    super::notes::add_heap_check(&mut output);
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
static EmelaTask tasks[4];static int count;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t,int32_t n) { assert(n>=1 && n<=4);tasks[n-1]=t;count++; }
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m);abort(); }
int main(void) {
 for(int i=0;i<1000;i++) {
  count=0;void *cause=NULL;EmelaFiber f=emela_export_work_0_start(&cause);
  while(emela_scheduler_run(1)) {} assert(count==4);
  int first=i%2 ? 1:3,second=i%2 ? 3:1;
  assert(emela_task_complete_int(tasks[first-1],0));while(emela_scheduler_run(1)) {}
  assert(!emela_fiber_done(f));
  assert(emela_task_complete_int(tasks[second-1],0));
  while(!emela_fiber_done(f)) { assert(emela_scheduler_run(1)); }
  assert(cause && !emela_task_complete_int(tasks[1],0) && !emela_task_complete_int(tasks[3],0));
  emela_fiber_release(f);
  char buffer[256],expected[256];
  int length=snprintf(expected,sizeof(expected),"error: Boom(n: %d)\nnote: another fiber also failed: error: Boom(n: %d)",first,second);
  EmelaString text=emela_export_show_1(cause,buffer,sizeof(buffer));
  assert(text.length==length && memcmp(text.data,expected,length)==0);
  emela_embedded_release(cause);assert(test_heap_empty());
 }
}
"#,
    );
}

#[test]
fn settled_nested_scope_failures_keep_their_notes_when_cleanup_raises_interrupt() {
    let source = r#"
import Std.Fiber.{Fiber, Success, Failure}
import Std.Cause.{Cause}
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
@external(c, "mark")
fn mark(n: Int) -> () use {}
error Boom(n: Int) derive Show
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting: Wait { suspend fn get(n,resume) { resume(wait(n)) } }
effect Resource { fn read() -> Int }
handler Res: Resource { fn read() { 0 }
 release { mark(2) }
}
fn cleanup(c: Cause[{}]) -> () { mark(1)
 Cause.raise(c)
 mark(99)
}
fn protected(c: Cause[{}]) -> Int use Wait { with Res {
 (use Wait).get(3)
 cleanup(c)
 0
} }
fn late(n: Int) -> Int fails Boom use Wait { (use Wait).get(n)
 fail Boom(n: n)
}
pub fn make() -> Cause[{}] use Async { with Waiting { Fiber.scope(fn() {
 child=Fiber.fork(fn() { (use Wait).get(4) })
 (use Wait).get(5)
 Fiber.interrupt(child)
 Fiber.await(child) match { Success(_) -> panic("unexpected")
  Failure(c) -> c
 }
}) } }
pub fn work(c: Cause[{}]) -> Cause[Boom] use Async { with Waiting {
 Fiber.attempt(fn() { Fiber.scope(fn() {
  child=Fiber.fork(fn() { Fiber.scope(fn() {
   Fiber.fork(fn() { late(1) })
   Fiber.scope(fn() {
    Fiber.fork(fn() { late(2) })
    protected(c)
   })
  }) })
  Fiber.await(child)
  0
 }) }) match { Success(_) -> panic("unexpected")
  Failure(c) -> c
 }
} }
pub fn show(c: Cause[Boom]) -> String { Cause.to_string(c) }
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Public).unwrap();
    let mut flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    super::interrupt::protect_cleanup_call(&mut flow);
    let (_, late) = flow
        .functions
        .iter_mut()
        .find(|(_, f)| f.name == "late")
        .unwrap();
    let hold = late.stmts.alloc(Stmt::Let {
        dst: None,
        on_defect: None,
        on_suspend: None,
        value: Value::Runtime {
            op: RuntimeOp::Hold,
            args: vec![],
        },
    });
    late.seqs[late.body.unwrap()].stmts.insert(0, hold);
    let mut output = compile_flow(
        &flow,
        &options()
            .with_heap_bytes(8192)
            .with_frame_capacity(48)
            .with_fiber_capacity(10)
            .with_task_capacity(3)
            .with_event_capacity(3),
    )
    .unwrap();
    super::notes::add_heap_check(&mut output);
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
static EmelaTask tasks[5];static int count,order;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t,int32_t n) { assert(n>=1 && n<=5);tasks[n-1]=t;count++; }
void mark(int32_t n) { assert(n==++order); }
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m);abort(); }
int main(void) {
 for(int i=0;i<1000;i++) {
  count=order=0;void *interrupt=NULL;EmelaFiber f=emela_export_make_0_start(&interrupt);
  while(emela_scheduler_run(1)) {} assert(count==2);
  assert(emela_task_complete_int(tasks[4],0));while(!emela_fiber_done(f)) { assert(emela_scheduler_run(1)); }
  emela_fiber_release(f);count=0;void *cause=NULL;
  f=emela_export_work_1_start(interrupt,i%3 ? &cause:NULL);
  while(emela_scheduler_run(1)) {} assert(count==3);
  int first=i%2 ? 2:1,second=3-first;
  assert(emela_task_complete_int(tasks[first-1],0));while(emela_scheduler_run(1)) {}
  assert(!emela_fiber_done(f) && order==2);
  assert(emela_task_complete_int(tasks[second-1],0));
  while(!emela_fiber_done(f)) { assert(emela_scheduler_run(1)); }
  assert(!emela_task_complete_int(tasks[2],0));emela_fiber_release(f);
  if(i%3) {
   char buffer[256],expected[256];int length=snprintf(expected,sizeof(expected),"error: Boom(n: %d)\nnote: another fiber also failed: error: Boom(n: %d)",first,second);
   EmelaString text=emela_export_show_2(cause,buffer,sizeof(buffer));
   assert(text.length==length && memcmp(text.data,expected,length)==0);emela_embedded_release(cause);
  }
  emela_embedded_release(interrupt);assert(test_heap_empty());
 }
}
"#,
    );
}
