use super::*;

#[test]
fn typed_task_queue_exhaustion_restores_interrupt_state_before_defect() {
    let mut core = emela_core::Module::new();
    let ty = core.tuple(vec![TypeId::INT64, TypeId::FLOAT]);
    let output = compile_flow(
        &echo_flow(core, ty),
        &options().with_heap_bytes(0).with_event_capacity(1),
    )
    .unwrap();
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask tasks[2];static int count,locked,produced,released;
uint32_t emela_board_event_lock(void) {{ int old=locked;locked=1;return old; }}
void emela_board_event_unlock(uint32_t p) {{ assert(locked);locked=(int)p; }}
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) {{
  (void)m;(void)n;exit(k==EMELA_DEFECT_HEAP_EXHAUSTED&&!locked&&!produced&&!released?0:2);
}}
void board_value_start(EmelaTask t,emela_type_{ty} input) {{ (void)input;assert(count<2);tasks[count++]=t; }}
static void produce(void*p,emela_type_{ty}*out) {{ (void)p;(void)out;produced++; }}
static void release(void*p) {{ (void)p;released++; }}
int main(void) {{
  emela_type_{ty} value={{0}};
  (void)emela_export_echo_1_start(value,NULL);(void)emela_export_echo_1_start(value,NULL);
  while(emela_scheduler_run(1)) {{}}assert(count==2);
  assert(emela_task_complete_value_{ty}(tasks[0],produce,NULL,release));
  (void)emela_task_complete_value_{ty}(tasks[1],produce,NULL,release);return 1;
}}
"#,
            ty = ty.index()
        ),
    );
}
