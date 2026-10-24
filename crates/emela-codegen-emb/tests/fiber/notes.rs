use super::*;
use emela_lir::Stmt;

fn masked_notes_flow() -> emela_lir::Module {
    let source = include_str!("../../examples/support/fiber_notes.emel");
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    let mut flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    // Hold lets all three verified Flow bodies fail despite sibling cancellation.
    // Each logical child ends while masked; no live caller inherits its hold count.
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
    flow
}

#[test]
fn later_failures_are_ordered_owned_notes_shared_by_rethrown_causes() {
    let flow = masked_notes_flow();
    let mut output = compile_flow(
        &flow,
        &options()
            .with_heap_bytes(4096)
            .with_frame_capacity(48)
            .with_task_capacity(4)
            .with_event_capacity(4),
    )
    .unwrap();
    add_heap_check(&mut output);
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
static EmelaTask tasks[4]; static int count;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t, int32_t n) { assert(n>=1 && n<=4);tasks[n-1]=t;count++; }
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m);abort(); }
int main(void) {
 for(int i=0;i<1000;i++) {
  void *cause=NULL;count=0;
  EmelaFiber f=emela_export_work_0_start(i%3 ? &cause : NULL);
  while(emela_scheduler_run(1)) {} assert(count==4);
  int order[3]={1,2,3}; if(i%2) { order[0]=3;order[2]=1; }
  for(int n=0;n<3;n++) {
   assert(emela_task_complete_int(tasks[order[n]-1],0));
   while(emela_scheduler_run(1)) {}
   assert(n==2 ? emela_fiber_done(f) : !emela_fiber_done(f));
  }
  assert(!emela_task_complete_int(tasks[3],0));emela_fiber_release(f);
  if(i%3) {
   void *again=NULL;
   if(i%2) { EmelaFiber g=emela_export_rethrow_async_2_start(cause,&again);
    while(!emela_fiber_done(g)) { assert(emela_scheduler_run(1)); }
    emela_fiber_release(g);
   } else again=emela_export_rethrow_1(cause);
   emela_embedded_release(cause);
   char buffer[256], expected[256];
   int length=snprintf(expected,sizeof(expected),"error: Boom(n: %d)\nnote: another fiber also failed: error: Boom(n: 2)\nnote: another fiber also failed: error: Boom(n: %d)",order[0],order[2]);
   EmelaString text=emela_export_show_3(again,buffer,sizeof(buffer));
   assert(text.data==buffer && text.length==length && memcmp(text.data,expected,length)==0);
   assert(emela_export_caught_4(again)==order[0]);
   emela_embedded_release(again);
  }
  assert(test_heap_empty());
 }
}
"#,
    );
}

#[test]
fn note_display_checks_the_callers_remaining_capacity() {
    let flow = masked_notes_flow();
    compile_and_run(
        compile_flow(
            &flow,
            &options()
                .with_frame_capacity(48)
                .with_task_capacity(4)
                .with_event_capacity(4),
        )
        .unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask tasks[4]; static int expected_defect;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t,int32_t n) { assert(n>=1 && n<=4);tasks[n-1]=t; }
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) {
 assert(expected_defect && k==EMELA_DEFECT_STRING_CAPACITY && n==24 && memcmp(m,"string capacity exceeded",24)==0);exit(0);
}
int main(void) {
 void *cause=NULL;EmelaFiber f=emela_export_work_0_start(&cause);
 while(emela_scheduler_run(1)) {}
 for(int i=0;i<3;i++) { assert(emela_task_complete_int(tasks[i],0));while(emela_scheduler_run(1)) {} }
 assert(emela_fiber_done(f) && cause);emela_fiber_release(f);
 char buffer[64];expected_defect=1;emela_export_show_3(cause,buffer,sizeof(buffer));abort();
}
"#,
    );
}

pub(super) fn add_heap_check(output: &mut Output) {
    output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents
        .push_str(
            r#"
int test_heap_empty(void) {
  if (!emela_heap_initialized) return 1;
  for (unsigned char *at = emela_heap_storage; at < emela_heap_end();) {
    emela_heap_block *block = (emela_heap_block *)(void *)at;
    if (!block->is_free || block->size == 0) return 0;
    at += block->size;
  }
  return 1;
}

"#,
        );
}
