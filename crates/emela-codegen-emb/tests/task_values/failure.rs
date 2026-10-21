use super::*;

#[test]
fn string_success_and_failure_notifications_are_exclusive_and_share_safe_context_ownership() {
    for indirect in [false, true] {
        let mut core = emela_core::Module::new();
        let error = core.add_enum(EnumDef {
            name: "Boom".into(),
            params: 0,
            variants: vec![VariantDef::positional("Boom", vec![TypeId::STRING])],
            error: true,
            external: false,
        });
        let error_ty = core.enum_type(error, vec![]);
        let callback = core.types.intern(Type::Fn(FnType {
            params: vec![TypeId::STRING],
            ret: TypeId::STRING,
            fails: true,
            suspends: true,
            ev: false,
            sets: None,
        }));
        let mut flow = Module::new(core);
        let mut b = FnBuilder::new(&flow.core, "value");
        b.set_suspends().set_fails().set_ret(TypeId::STRING);
        b.param("input", TypeId::STRING);
        let target = flow.add(b.finish_external(External {
            bindings: vec![ExternalBinding {
                target: "c".into(),
                host: HostRef::Global("host_value".into()),
            }],
        }));
        let mut b = FnBuilder::new(&flow.core, "caught");
        b.set_exported().set_suspends().set_ret(TypeId::STRING);
        let input = b.param("input", TypeId::STRING);
        let callee = if indirect {
            Callee::Indirect(b.assign("callback", callback, Value::FnRef(target)))
        } else {
            Callee::Direct(target)
        };
        let caught = b.var("caught", TypeId::ANY);
        b.block(Some(caught), "caught", |b, label| {
            let value = b.assign(
                "value",
                TypeId::STRING,
                Value::Call {
                    callee,
                    args: vec![input.into()],
                    suspends: true,
                    on_fail: Some(OnFail::Break(label)),
                },
            );
            b.term(Term::Return(Some(value.into())));
        });
        let typed = b.assign(
            "typed",
            error_ty,
            Value::AsError {
                enum_id: error,
                error: caught,
            },
        );
        let text = b.assign(
            "text",
            TypeId::STRING,
            Value::Field {
                ctor: CtorRef {
                    enum_id: error,
                    variant: 0,
                },
                base: typed,
                index: 0,
            },
        );
        b.term(Term::Return(Some(text.into())));
        flow.add(b.finish());
        let mut output = compile_flow(&flow, &options().with_heap_bytes(1024)).unwrap();
        managed::add_heap_check(&mut output);
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
typedef struct {{ EmelaString argument;char bytes[3]; }} Context;
static EmelaTask task;static Context context;static int produced,released,isr;
uint32_t emela_board_event_lock(void) {{ return 0; }}
void emela_board_event_unlock(uint32_t p) {{ (void)p; }}
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) {{ (void)k;(void)m;(void)n;abort(); }}
void board_value_start(EmelaTask t,EmelaString input) {{ task=t;context.argument=input;emela_embedded_retain(input.owner);memcpy(context.bytes,"a\0b",3); }}
static void success(void*p,EmelaString*out) {{ Context*c=p;assert(!isr);produced++;*out=(EmelaString){{.data=c->bytes,.length=3}}; }}
static void failure(void*p,EmelaError*out) {{ Context*c=p;assert(!isr);produced++;
  *out=(EmelaError){{.tag=1,.payload.e0=(emela_type_{error_ty}){{.tag=0,.payload.v0.f0={{.data=c->bytes,.length=3}}}}}};
}}
static void release(void*p) {{ Context*c=p;assert(!isr);released++;memset(c->bytes,'z',3);emela_embedded_release(c->argument.owner); }}
int main(void) {{ for(int i=0;i<1000;i++) {{
  char original[]="arg";EmelaString result={{0}};produced=released=0;
  EmelaFiber f=emela_export_caught_1_start((EmelaString){{.data=original,.length=3}},i%3?&result:NULL);
  memset(original,'x',3);while(emela_scheduler_run(1)) {{}}
  assert(context.argument.owner&&!memcmp(context.argument.data,"arg",3));isr=1;
  if(i%2) {{ assert(emela_task_fail(task,failure,&context,release));assert(!emela_task_complete_value_{string}(task,success,&context,release)); }}
  else {{ assert(emela_task_complete_value_{string}(task,success,&context,release));assert(!emela_task_fail(task,failure,&context,release)); }}
  assert(!produced&&!released);isr=0;
  while(!emela_fiber_done(f)) {{ assert(emela_scheduler_run(1)); }}emela_fiber_release(f);
  assert(produced==1&&released==1);if(i%3) {{ assert(result.owner&&result.length==3&&!memcmp(result.data,"a\0b",3));emela_embedded_release(result.owner); }}
  assert(test_heap_empty());
}} }}
"#,
                error_ty = error_ty.index(),
                string = TypeId::STRING.index()
            ),
        );
    }
}
