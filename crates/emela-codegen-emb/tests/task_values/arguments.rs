use super::*;

#[test]
fn non_scalar_arguments_work_with_the_existing_scalar_completion_abi() {
    let mut flow = Module::new(emela_core::Module::new());
    let mut b = FnBuilder::new(&flow.core, "consume");
    b.set_suspends();
    b.param("text", TypeId::STRING);
    let target = flow.add(b.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("host_value".into()),
        }],
    }));
    let mut b = FnBuilder::new(&flow.core, "work");
    b.set_exported().set_suspends();
    let text = b.param("text", TypeId::STRING);
    b.let_(
        None,
        Value::Call {
            callee: Callee::Direct(target),
            args: vec![text.into()],
            suspends: true,
            on_fail: None,
        },
    );
    b.term(Term::Return(None));
    flow.add(b.finish());
    let mut output = compile_flow(&flow, &options().with_heap_bytes(512)).unwrap();
    assert!(
        !output
            .files
            .iter()
            .find(|f| f.path == "emela_embedded.c")
            .unwrap()
            .contents
            .contains("#define EMELA_TASK_VALUE_RUNTIME")
    );
    managed::add_heap_check(&mut output);
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
static EmelaTask task;static EmelaString saved;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) { (void)k;(void)m;(void)n;abort(); }
void board_value_start(EmelaTask t,EmelaString input) { task=t;saved=input;emela_embedded_retain(saved.owner); }
int main(void) { for(int i=0;i<1000;i++) {
  char bytes[]={'a','\0','b'};EmelaFiber f=emela_export_work_1_start((EmelaString){.data=bytes,.length=3});
  memset(bytes,'z',3);while(emela_scheduler_run(1)) {}
  assert(saved.owner&&saved.length==3&&!memcmp(saved.data,"a\0b",3));assert(emela_task_complete_unit(task));
  while(!emela_fiber_done(f)) { assert(emela_scheduler_run(1)); }emela_fiber_release(f);
  assert(saved.length==3&&!memcmp(saved.data,"a\0b",3));emela_embedded_release(saved.owner);assert(test_heap_empty());
} }
"#,
    );
}
