use super::*;

#[test]
fn exported_failing_external_successes_and_failures_follow_the_public_entry_policy() {
    for mode in 0..4 {
        let mut output = compile_flow(
            &failure::flow(),
            &Options::default()
                .with_heap_bytes(1024)
                .with_frame_bytes(512)
                .with_external_adapter(
                    "host_failure",
                    ExternalAdapter::new("board_failure", ExternalResult::Borrowed),
                ),
        )
        .unwrap();
        add_heap_check(&mut output);
        let tag = if mode == 2 { 999 } else { 1 };
        let variant = if mode == 3 { 999 } else { 0 };
        let failure = if mode == 0 {
            String::new()
        } else {
            format!(
                "*error=(EmelaError){{.tag={tag},.payload.e0={{.tag={variant},.payload.v0.f0=input}}}};"
            )
        };
        let expected = if mode == 1 {
            "EMELA_DEFECT_UNHANDLED_ERROR"
        } else {
            "EMELA_DEFECT_ASSERT"
        };
        let invoke = if mode == 0 {
            "for(int i=0;i<1000;i++) {EmelaString text=emela_export_failure_0((EmelaString){.data=\"a\\0b\",.length=3});assert(text.owner&&text.length==3&&!memcmp(text.data,\"a\\0b\",3));emela_embedded_release_string(&text);assert(test_heap_empty());}return 0;"
        } else {
            "(void)emela_export_failure_0((EmelaString){.data=\"abc\",.length=3});return 1;"
        };
        compile_and_run(
            output,
            &format!(
                r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) {{(void)m;(void)n;exit(k=={expected}?0:2);}}
EmelaString board_failure(EmelaString input,EmelaError *error) {{(void)error;{failure}return input;}}
int main(void) {{{invoke}}}
"#
            ),
        );
    }
}

#[test]
fn exported_suspend_external_entries_keep_borrowed_arguments_and_null_results_safe() {
    let mut flow = Module::new(emela_core::Module::new());
    let mut b = FnBuilder::new(&flow.core, "entry");
    b.set_exported().set_suspends().set_ret(TypeId::STRING);
    b.param("text", TypeId::STRING);
    flow.add(b.finish_external(binding("host_task")));
    let mut output = compile_flow(
        &flow,
        &Options::default()
            .with_heap_bytes(1024)
            .with_frame_bytes(512)
            .with_task_adapter("host_task", "board_task_start"),
    )
    .unwrap();
    add_heap_check(&mut output);
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);static EmelaTask task;static EmelaString held;
uint32_t emela_board_event_lock(void) {return 0;}void emela_board_event_unlock(uint32_t p) {(void)p;}
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) {(void)k;(void)m;(void)n;abort();}
void board_task_start(EmelaTask pending,EmelaString input) {task=pending;held=input;emela_embedded_retain_string(&held);}
static void produce(void *p,EmelaString *out) {(void)p;*out=held;}
static void release(void *p) {(void)p;emela_embedded_release_string(&held);}
int main(void) {for(int i=0;i<1000;i++) {
 for(int discard=0;discard<2;discard++) {char bytes[]={'a','\0','b'};EmelaString out={0};EmelaFiber f=emela_export_entry_0_start((EmelaString){.data=bytes,.length=3},discard?NULL:&out);
  memset(bytes,'x',3);while(emela_scheduler_run(1)) {}assert(!emela_fiber_done(f));assert(held.owner&&held.length==3&&!memcmp(held.data,"a\0b",3));assert(emela_task_complete_value_4(task,produce,NULL,release));
  while(!emela_fiber_done(f)) {assert(emela_scheduler_run(1));}emela_fiber_release(f);if(!discard) {assert(out.owner&&out.length==3&&!memcmp(out.data,"a\0b",3));emela_embedded_release_string(&out);}assert(test_heap_empty());
 }
}}
"#,
    );
}
