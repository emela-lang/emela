use super::*;

#[test]
fn string_buffer_adapters_use_caller_storage_and_own_indirect_and_suspended_results() {
    let (flow, _, _, _) = values::fixture(values::Shape::String);
    let mut output = compile_flow(
        &flow,
        &Options::default()
            .with_heap_bytes(2048)
            .with_frame_bytes(1024)
            .with_external_adapter(
                "host_roundtrip",
                ExternalAdapter::new("board_roundtrip", ExternalResult::StringBuffer),
            ),
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
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) { (void)k;(void)m;(void)n;abort(); }
EmelaString board_roundtrip(EmelaString input,char *output,uint16_t capacity) {
 assert(input.length<=capacity && (input.length==0 || output!=NULL));
 if(input.length) memmove(output,input.data,input.length);
 return (EmelaString){.data=output,.length=input.length};
}
static void check(EmelaString value) { assert(value.length==3&&!memcmp(value.data,"a\0b",3));emela_embedded_release_string(&value); }
int main(void) {for(int i=0;i<1000;i++) {
 EmelaString input={.data="a\0b",.length=3};char output[3];
 EmelaString value=emela_export_external_0(input,output,sizeof output);assert(value.data==output&&!value.owner);check(value);
 value=emela_export_direct_1(input,output,sizeof output);assert(value.data==output&&!value.owner);check(value);
 check(emela_export_indirect_2(input));
 value=(EmelaString){0};EmelaFiber f=emela_export_coerced_3_start(input,&value);while(!emela_fiber_done(f)) {assert(emela_scheduler_run(1));}emela_fiber_release(f);check(value);
 value=(EmelaString){0};f=emela_export_suspended_4_start(input,&value);while(!emela_fiber_done(f)) {assert(emela_scheduler_run(1));}emela_fiber_release(f);check(value);
 f=emela_export_suspended_4_start(input,NULL);while(!emela_fiber_done(f)) {assert(emela_scheduler_run(1));}emela_fiber_release(f);
 emela_export_discard_5(input);
 value=emela_export_external_0((EmelaString){0},NULL,0);assert(!value.length&&!value.owner&&!value.data);assert(test_heap_empty());
}}
"#,
    );
}

#[test]
fn string_buffer_adapters_stay_heapless_and_reject_capacity_and_invalid_outputs() {
    let mut flow = Module::new(emela_core::Module::new());
    let mut b = FnBuilder::new(&flow.core, "entry");
    b.set_exported().set_ret(TypeId::STRING);
    b.param("value", TypeId::STRING);
    flow.add(b.finish_external(binding("host_buffer")));
    for mode in 0..4 {
        let output = compile_flow(
            &flow,
            &Options::default().with_external_adapter(
                "host_buffer",
                ExternalAdapter::new("board_buffer", ExternalResult::StringBuffer),
            ),
        )
        .unwrap();
        assert!(
            !output
                .files
                .iter()
                .any(|f| f.contents.contains("emela_heap_storage"))
        );
        let body = match mode {
            0 => {
                "if (input.length>capacity) emela_board_defect(EMELA_DEFECT_STRING_CAPACITY,\"capacity\",8);return (EmelaString){.data=output,.length=input.length};"
            }
            1 => "return (EmelaString){.data=input.data,.length=input.length};",
            2 => {
                "return (EmelaString){.data=output,.length=input.length,.owner=(void*)(uintptr_t)1};"
            }
            _ => "return (EmelaString){.data=output,.length=(uint16_t)(capacity+1)};",
        };
        let capacity = if mode == 0 { 2 } else { 3 };
        compile_and_run(
            output,
            &format!(
                r#"
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) {{(void)m;(void)n;exit(k==EMELA_DEFECT_STRING_CAPACITY?0:2);}}
EmelaString board_buffer(EmelaString input,char *output,uint16_t capacity) {{(void)input;(void)output;(void)capacity;{body}}}
int main(void) {{char output[3];(void)emela_export_entry_0((EmelaString){{.data="abc",.length=3}},output,{capacity});return 1;}}
"#
            ),
        );
    }
}
