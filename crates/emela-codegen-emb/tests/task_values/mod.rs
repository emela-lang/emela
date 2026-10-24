use super::*;
mod arguments;
mod error_any;
mod failure;
mod managed;
mod queue;
mod representations;

fn options() -> Options {
    Options::default()
        .with_heap_bytes(4096)
        .with_fiber_capacity(8)
        .with_frame_capacity(24)
        .with_frame_bytes(1024)
        .with_deferred_capacity(2)
        .with_task_capacity(2)
        .with_event_capacity(2)
        .with_task_adapter("host_value", "board_value_start")
}

fn echo_flow(core: emela_core::Module, ty: TypeId) -> Module {
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "value");
    b.set_suspends().set_ret(ty);
    b.param("input", ty);
    let target = flow.add(b.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("host_value".into()),
        }],
    }));
    let mut b = FnBuilder::new(&flow.core, "echo");
    b.set_exported().set_suspends().set_ret(ty);
    let input = b.param("input", ty);
    let result = b.assign(
        "result",
        ty,
        Value::Call {
            callee: Callee::Direct(target),
            args: vec![input.into()],
            suspends: true,
            on_fail: None,
        },
    );
    b.term(Term::Return(Some(result.into())));
    flow.add(b.finish());
    flow
}

#[test]
fn typed_task_producers_return_inline_aggregates_without_heap_or_function_pointer_casts() {
    let mut core = emela_core::Module::new();
    let ty = core.tuple(vec![TypeId::INT64, TypeId::FLOAT]);
    let output = compile_flow(&echo_flow(core, ty), &options().with_heap_bytes(0)).unwrap();
    assert!(
        !output
            .files
            .iter()
            .find(|f| f.path == "emela_embedded.c")
            .unwrap()
            .contents
            .contains("emela_heap_storage")
    );
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask pending;
static emela_type_{ty} saved;
static int locked,isr,produced,released;
uint32_t emela_board_event_lock(void) {{ int old=locked;locked=1;return old; }}
void emela_board_event_unlock(uint32_t p) {{ assert(locked);locked=(int)p; }}
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) {{ (void)k;(void)m;(void)n;abort(); }}
void board_value_start(EmelaTask task,emela_type_{ty} input) {{ pending=task;saved=input; }}
static void produce(void*p,emela_type_{ty}*out) {{ assert(!locked&&!isr);produced++;*out=*(emela_type_{ty}*)p; }}
static void release(void*p) {{ assert(!locked&&!isr);released++;memset(p,0,sizeof(emela_type_{ty})); }}
int main(void) {{
  for(int i=0;i<1000;i++) {{
    const uint64_t raw=UINT64_C(0xfff8000000000042);uint64_t bits;
    emela_type_{ty} input={{.f0=INT64_MIN}},output={{0}};memcpy(&input.f1,&raw,sizeof(raw));
    produced=released=0;EmelaFiber f=emela_export_echo_1_start(input,i%2?NULL:&output);
    while(emela_scheduler_run(1)) {{}}
    isr=1;assert(!emela_task_complete_int(pending,1));assert(!emela_task_complete_value_{ty}(pending,NULL,&saved,release));
    assert(emela_task_complete_value_{ty}(pending,produce,&saved,release));
    assert(!emela_task_complete_value_{ty}(pending,produce,&saved,release));assert(!produced&&!released);isr=0;
    while(!emela_fiber_done(f)) {{ assert(emela_scheduler_run(1)); }}
    emela_fiber_release(f);
    assert(produced==1&&released==1);if(!(i%2)) {{ memcpy(&bits,&output.f1,sizeof(bits));assert(output.f0==INT64_MIN&&bits==raw); }}
    assert(!emela_task_complete_value_{ty}(pending,produce,&saved,release));
  }}
}}
"#,
            ty = ty.index()
        ),
    );
}

#[test]
fn external_task_signatures_reject_callbacks_even_when_nested_in_aggregates() {
    for nested in [false, true] {
        let mut core = emela_core::Module::new();
        let callback = core
            .types
            .intern(Type::Fn(FnType::pure(vec![], TypeId::INT)));
        let ty = if nested {
            core.tuple(vec![TypeId::STRING, callback])
        } else {
            callback
        };
        let errors = compile_flow(&echo_flow(core, ty), &options()).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("callback or runtime capability"))
        );
    }
}
