use super::*;

pub(super) fn flow() -> Module {
    let mut core = emela_core::Module::new();
    let error = core.add_enum(EnumDef {
        name: "Boom".into(),
        params: 0,
        variants: vec![VariantDef::positional("Boom", vec![TypeId::STRING])],
        error: true,
        external: false,
    });
    let error_ty = core.enum_type(error, vec![]);
    let callback_ty = core.types.intern(Type::Fn(FnType {
        fails: true,
        ..FnType::pure(vec![TypeId::STRING], TypeId::STRING)
    }));
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "failure");
    b.set_exported().set_fails().set_ret(TypeId::STRING);
    b.param("text", TypeId::STRING);
    let target = flow.add(b.finish_external(binding("host_failure")));
    for (index, name) in ["direct", "indirect", "discard", "suspended"]
        .into_iter()
        .enumerate()
    {
        let mut b = FnBuilder::new(&flow.core, name);
        b.set_exported().set_ret(TypeId::STRING);
        if index == 3 {
            b.set_suspends();
        }
        let text = b.param("text", TypeId::STRING);
        let callee = if index == 1 {
            Callee::Indirect(b.assign("callback", callback_ty, Value::FnRef(target)))
        } else {
            Callee::Direct(target)
        };
        let caught = b.var("caught", TypeId::ANY);
        b.block(Some(caught), "escape", |b, label| {
            let value = Value::Call {
                callee,
                args: vec![text.into()],
                suspends: false,
                on_fail: Some(OnFail::Break(label)),
            };
            if index == 2 {
                b.let_(None, value);
            } else {
                b.assign("ignored", TypeId::STRING, value);
            }
            b.term(Term::Defect(
                Const::String("expected failure".into()).into(),
            ));
        });
        let concrete = b.assign(
            "concrete",
            error_ty,
            Value::AsError {
                error: caught,
                enum_id: error,
            },
        );
        let message = b.assign(
            "message",
            TypeId::STRING,
            Value::Field {
                base: concrete,
                ctor: CtorRef {
                    enum_id: error,
                    variant: 0,
                },
                index: 0,
            },
        );
        b.term(Term::Return(Some(message.into())));
        flow.add(b.finish());
    }
    flow
}

#[test]
fn failing_generated_value_adapters_own_errors_and_ignore_the_failed_success_channel() {
    for result in [ExternalResult::Borrowed, ExternalResult::Owned] {
        let mut output = compile_flow(
            &flow(),
            &Options::default()
                .with_heap_bytes(2048)
                .with_frame_bytes(512)
                .with_external_adapter(
                    "host_failure",
                    ExternalAdapter::new("board_failure", result),
                ),
        )
        .unwrap();
        add_heap_check(&mut output);
        let own = if result == ExternalResult::Owned {
            "emela_external_own_error(error);"
        } else {
            ""
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
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) {{ (void)k;(void)m;(void)n;abort(); }}
EmelaString board_failure(EmelaString input,EmelaError *error) {{
 *error=(EmelaError){{.tag=1,.payload.e0={{.tag=0,.payload.v0.f0=input}}}};{own}
 return (EmelaString){{.data=(const char*)(uintptr_t)1,.length=UINT16_MAX,.owner=(void*)(uintptr_t)1}};
}}
static void check(EmelaString value) {{ assert(value.owner&&value.length==3&&!memcmp(value.data,"a\0b",3));emela_embedded_release_string(&value); }}
int main(void) {{ for(int i=0;i<1000;i++) {{
 char bytes[]={{'a','\0','b'}};EmelaString input={{.data=bytes,.length=3}};
 EmelaString value=emela_export_direct_1(input);memset(bytes,'x',3);check(value);assert(test_heap_empty());
 input=(EmelaString){{.data="a\0b",.length=3}};
 check(emela_export_indirect_2(input));assert(test_heap_empty());
 check(emela_export_discard_3(input));assert(test_heap_empty());
 value=(EmelaString){{0}};EmelaFiber f=emela_export_suspended_4_start(input,&value);
 while(!emela_fiber_done(f)) {{ assert(emela_scheduler_run(1)); }}emela_fiber_release(f);check(value);assert(test_heap_empty());
 f=emela_export_suspended_4_start(input,NULL);while(!emela_fiber_done(f)) {{ assert(emela_scheduler_run(1)); }}emela_fiber_release(f);assert(test_heap_empty());
}} }}
"#
            ),
        );
    }
}
