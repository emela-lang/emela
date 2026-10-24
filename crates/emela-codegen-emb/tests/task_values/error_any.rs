use super::*;

use crate::gc_anchor;

#[test]
fn tagged_error_any_successes_and_any_fields_snapshot_strings_and_keep_list_owners() {
    for (field, with_collector) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut core = emela_core::Module::new();
        let list = core.list(TypeId::STRING);
        let error = core.add_enum(EnumDef {
            name: "Boom".into(),
            params: 0,
            variants: vec![VariantDef::positional("Boom", vec![TypeId::STRING, list])],
            error: true,
            external: false,
        });
        let error_ty = core.enum_type(error, vec![]);
        let ret = if field {
            core.tuple(vec![TypeId::ANY, TypeId::BOOL])
        } else {
            TypeId::ANY
        };
        let mut flow = echo_flow(core, ret);
        let mut b = FnBuilder::new(&flow.core, "seed");
        b.set_exported().set_ret(list);
        let text = b.param("text", TypeId::STRING);
        let parts = b.assign(
            "parts",
            list,
            Value::List {
                elems: vec![text.into()],
                tail: None,
            },
        );
        b.term(Term::Return(Some(parts.into())));
        flow.add(b.finish());
        let mut b = FnBuilder::new(&flow.core, "read");
        b.set_exported().set_ret(TypeId::INT);
        let input = b.param("input", ret);
        let any = if field {
            b.assign(
                "any",
                TypeId::ANY,
                Value::TupleGet {
                    base: input,
                    index: 0,
                },
            )
        } else {
            input
        };
        let typed = b.assign(
            "typed",
            error_ty,
            Value::AsError {
                enum_id: error,
                error: any,
            },
        );
        let message = b.assign(
            "message",
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
        let parts = b.assign(
            "parts",
            list,
            Value::Field {
                ctor: CtorRef {
                    enum_id: error,
                    variant: 0,
                },
                base: typed,
                index: 1,
            },
        );
        let head = b.assign("head", TypeId::STRING, Value::ListHead(parts));
        let length = b.assign(
            "length",
            TypeId::INT,
            Value::Builtin {
                op: Builtin::StringByteSize,
                args: vec![message.into()],
            },
        );
        let part_length = b.assign(
            "part_length",
            TypeId::INT,
            Value::Builtin {
                op: Builtin::StringByteSize,
                args: vec![head.into()],
            },
        );
        let total = b.assign(
            "total",
            TypeId::INT,
            Value::Prim {
                op: PrimOp::Add,
                ty: PrimTy::I32,
                args: vec![length.into(), part_length.into()],
            },
        );
        b.term(Term::Return(Some(total.into())));
        flow.add(b.finish());
        if with_collector {
            gc_anchor::add(&mut flow);
        }
        let mut output = compile_flow(&flow, &options().with_heap_bytes(2048)).unwrap();
        managed::add_heap_check(&mut output);
        assert_eq!(
            output
                .files
                .iter()
                .find(|file| file.path == "emela_embedded.c")
                .unwrap()
                .contents
                .contains("static void emela_heap_collect_cycles("),
            with_collector,
        );
        let source = &mut output
            .files
            .iter_mut()
            .find(|file| file.path == "emela_embedded.c")
            .unwrap()
            .contents;
        if with_collector {
            *source = source.replace(
                "static void emela_heap_collect_cycles(void) {",
                "static unsigned test_collections_count;\nstatic void emela_heap_collect_cycles(void) {\n  ++test_collections_count;",
            );
            source.push_str("unsigned test_collections(void) { return test_collections_count; }\n");
        } else {
            source.push_str("unsigned test_collections(void) { return 0; }\n");
        }
        let header = &output
            .files
            .iter()
            .find(|f| f.path == "emela_embedded.h")
            .unwrap()
            .contents;
        let released_ty = if field { ret } else { error_ty };
        let release: usize = header
            .lines()
            .find(|l| {
                l.starts_with("void emela_embedded_release_value_")
                    && l.contains(&format!("emela_type_{} *", released_ty.index()))
            })
            .unwrap()
            .trim_start_matches("void emela_embedded_release_value_")
            .split('(')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let c_type = if field {
            format!("emela_type_{}", ret.index())
        } else {
            "EmelaAny".into()
        };
        let assign = if field {
            format!("*out=(emela_type_{}){{.f0=borrowed,.f1=1}};", ret.index())
        } else {
            "*out=borrowed;".into()
        };
        let view = if field { "result.f0" } else { "result" };
        let release_result = if field {
            format!("emela_embedded_release_value_{release}(&result);")
        } else {
            format!("emela_embedded_release_value_{release}(&result.payload.e0);")
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
unsigned test_collections(void);
typedef struct {{ char bytes[3];void*parts; }} Context;
static EmelaTask task;static int produced,released;
uint32_t emela_board_event_lock(void) {{ return 0; }}
void emela_board_event_unlock(uint32_t p) {{ (void)p; }}
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) {{ (void)k;(void)m;(void)n;abort(); }}
void board_value_start(EmelaTask t,{c_type} input) {{ (void)input;task=t; }}
static void produce(void*p,{c_type}*out) {{ Context*c=p;produced++;
  EmelaAny borrowed={{.tag=1,.payload.e0=(emela_type_{error_ty}){{.tag=0,.payload.v0={{
    .f0={{.data=c->bytes,.length=3}},.f1=c->parts
  }}}}}};{assign}
}}
static void release(void*p) {{ Context*c=p;released++;memset(c->bytes,'z',3);
  unsigned collections=test_collections();emela_embedded_release(c->parts);
  if({gc}) assert(test_collections()>collections);
}}
int main(void) {{ for(int i=0;i<1000;i++) {{
  Context context={{.bytes={{'a','\0','b'}},.parts=emela_export_seed_2((EmelaString){{.data="42",.length=2}})}};
  {c_type} input={{0}},result={{0}};produced=released=0;
  EmelaFiber f=emela_export_echo_1_start(input,i%2?NULL:&result);while(emela_scheduler_run(1)) {{}}
  assert(emela_task_complete_value_{ret}(task,produce,&context,release));
  while(!emela_fiber_done(f)) {{ assert(emela_scheduler_run(1)); }}emela_fiber_release(f);
  assert(produced==1&&released==1);if(!(i%2)) {{
    EmelaAny view={view};EmelaString text=view.payload.e0.payload.v0.f0;
    assert(view.tag==1&&text.owner&&text.length==3&&!memcmp(text.data,"a\0b",3));
    assert(emela_export_read_3(result)==5);{release_result}
  }}assert(test_heap_empty());
}} }}
"#,
                ret = ret.index(),
                error_ty = error_ty.index(),
                gc = usize::from(with_collector),
            ),
        );
    }
}
