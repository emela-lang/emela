use super::*;

#[test]
fn attempt_captures_sync_failure_and_returns_owned_cause() {
    let source = r#"
import Std.Fiber.{Fiber, Success, Failure}
error Boom(n: Int)
pub fn attempt(n: Int) -> Int {
 a = Fiber.attempt(fn() { if n == 0 { 41 } else { fail Boom(n: n) } })
 a match { Success(v) -> v + 1
  Failure(_) -> 42
 }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    compile_and_run(
        compile(&core, &options().with_heap_bytes(1024)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m);abort(); }
int main(void) { for(int i=0;i<1000;i++) assert(emela_export_attempt_0(i%2)==42); }
"#,
    );
}

#[test]
fn attempt_waits_for_async_cleanup_before_catching_failure() {
    let source = r#"
import Std.Fiber.{Fiber, Success, Failure}
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
@external(c, "mark")
fn mark(n: Int) -> () use {}
error Boom(n: Int)
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting: Wait { suspend fn get(n,resume) { resume(wait(n)) } }
effect Resource { fn read() -> Int }
handler Res: Resource {
 fn read() { 0 }
 release { mark(1)
  (use Wait).get(2)
  mark(2)
 }
}
pub fn attempt(n: Int) -> Int use Async {
 with Waiting {
  a = Fiber.attempt(fn() { with Res {
   x = (use Wait).get(1)
   if n == 0 { x } else { fail Boom(n: n) }
  } })
  mark(3)
  a match { Success(v) -> v
   Failure(_) -> 42
  }
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
static int marks, waits;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t,int32_t n) { assert(n==++waits);task=t; }
void mark(int32_t n) { assert(n==++marks); }
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m);abort(); }
int main(void) {
 for(int i=0;i<1000;i++) {
  marks=waits=0;int32_t result=-1;
  EmelaFiber f=emela_export_attempt_0_start(i%2,&result);
  while(emela_scheduler_run(1)) {}
  assert(waits==1 && marks==0);
  assert(emela_task_complete_int(task,42));
  while(emela_scheduler_run(1)) {}
  assert(waits==2 && marks==1 && result==-1);
  assert(emela_task_complete_int(task,0));
  while(!emela_fiber_done(f)) assert(emela_scheduler_run(1));
  assert(marks==3 && result==42);emela_fiber_release(f);
 }
}
"#,
    );
}

#[test]
fn attempt_owns_managed_success_and_releases_discarded_results() {
    let source = r#"
import Std.Fiber.{Fiber, Success, Failure}
error Boom
pub fn attempt(n: Int) -> Int {
 Fiber.attempt(fn() { fail Boom })
 Fiber.attempt(fn() { [n, n+1] })
 a = Fiber.attempt(fn() { [n, n+1] })
 a match { Success(xs) -> List.length(xs) + 40
  Failure(_) -> -1
 }
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
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m);abort(); }
int main(void) { for(int i=0;i<1000;i++) assert(emela_export_attempt_0(42)==42); }
"#,
    );
}

#[test]
fn attempt_keeps_the_embedded_policy_of_stopping_immediately_on_defect() {
    let source = r#"
import Std.Fiber.{Fiber, Success, Failure}
@external(c, "mark")
fn mark(n: Int) -> () use {}
effect Resource { fn read() -> Int }
handler Res: Resource { fn read() { 0 }
 release { mark(99) }
}
pub fn stopped() -> Int {
 Fiber.attempt(fn() { with Res { panic("test defect") } }) match {
  Success(_) -> 1
  Failure(_) -> 2
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
void mark(int32_t n) { (void)n;abort(); }
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) {
 assert(k==EMELA_DEFECT_PANIC && n==11 && memcmp(m,"test defect",11)==0);exit(0);
}
int main(void) { (void)emela_export_stopped_0();abort(); }
"#,
    );
}
