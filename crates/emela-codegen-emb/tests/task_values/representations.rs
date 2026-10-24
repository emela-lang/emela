use super::*;

#[test]
fn list_successes_keep_their_owned_elements_after_input_and_board_refs_are_released() {
    let mut core = emela_core::Module::new();
    let ty = core.list(TypeId::STRING);
    let mut flow = echo_flow(core, ty);
    let mut b = FnBuilder::new(&flow.core, "make");
    b.set_exported().set_ret(ty);
    let text = b.param("text", TypeId::STRING);
    let list = b.assign(
        "list",
        ty,
        Value::List {
            elems: vec![text.into()],
            tail: None,
        },
    );
    b.term(Term::Return(Some(list.into())));
    flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "read");
    b.set_exported().set_ret(TypeId::INT);
    let list = b.param("list", ty);
    let head = b.assign("head", TypeId::STRING, Value::ListHead(list));
    let length = b.assign(
        "length",
        TypeId::INT,
        Value::Builtin {
            op: Builtin::StringByteSize,
            args: vec![head.into()],
        },
    );
    b.term(Term::Return(Some(length.into())));
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
static EmelaTask task;static void*saved;static int produced,released;
uint32_t emela_board_event_lock(void) {{ return 0; }}
void emela_board_event_unlock(uint32_t p) {{ (void)p; }}
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) {{ (void)k;(void)m;(void)n;abort(); }}
void board_value_start(EmelaTask t,void*input) {{ task=t;saved=input;emela_embedded_retain(input); }}
static void produce(void*p,void**out) {{ produced++;*out=p; }}
static void release(void*p) {{ released++;emela_embedded_release(p); }}
int main(void) {{ for(int i=0;i<1000;i++) {{
  char bytes[]={{'a','\0','b'}};void*input=emela_export_make_2((EmelaString){{.data=bytes,.length=3}}),*result=NULL;
  memset(bytes,'z',3);produced=released=0;EmelaFiber f=emela_export_echo_1_start(input,i%2?NULL:&result);
  emela_embedded_release(input);while(emela_scheduler_run(1)) {{}}
  assert(emela_task_complete_value_{ty}(task,produce,saved,release));
  while(!emela_fiber_done(f)) {{ assert(emela_scheduler_run(1)); }}
  emela_fiber_release(f);assert(produced==1&&released==1);
  if(!(i%2)) {{ assert(emela_export_read_3(result)==3);emela_embedded_release(result); }}
  assert(test_heap_empty());
}} }}
"#,
            ty = ty.index()
        ),
    );
}

#[test]
fn boxed_any_successes_use_the_generated_ownership_representation() {
    let mut flow = echo_flow(emela_core::Module::new(), TypeId::ANY);
    let mut b = FnBuilder::new(&flow.core, "make");
    b.set_exported().set_ret(TypeId::ANY);
    let n = b.param("n", TypeId::INT);
    let boxed = b.assign("boxed", TypeId::ANY, Value::Coerce(n));
    b.term(Term::Return(Some(boxed.into())));
    flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "read");
    b.set_exported().set_ret(TypeId::INT);
    let boxed = b.param("boxed", TypeId::ANY);
    let n = b.assign("n", TypeId::INT, Value::Coerce(boxed));
    b.term(Term::Return(Some(n.into())));
    flow.add(b.finish());
    let mut output = compile_flow(&flow, &options().with_heap_bytes(1024)).unwrap();
    managed::add_heap_check(&mut output);
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
static EmelaTask task;static EmelaAny saved;
uint32_t emela_board_event_lock(void) {{ return 0; }}
void emela_board_event_unlock(uint32_t p) {{ (void)p; }}
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) {{ (void)k;(void)m;(void)n;abort(); }}
void board_value_start(EmelaTask t,EmelaAny input) {{ assert(input.tag==0);task=t;saved=input;emela_embedded_retain(input.payload.any_box); }}
static void produce(void*p,EmelaAny*out) {{ *out=*(EmelaAny*)p; }}
static void release(void*p) {{ EmelaAny*v=p;emela_embedded_release(v->payload.any_box);*v=(EmelaAny){{0}}; }}
int main(void) {{ for(int i=0;i<1000;i++) {{
  EmelaAny input=emela_export_make_2(i),result={{0}};EmelaFiber f=emela_export_echo_1_start(input,i%2?NULL:&result);
  emela_embedded_release(input.payload.any_box);while(emela_scheduler_run(1)) {{}}
  assert(emela_task_complete_value_{ty}(task,produce,&saved,release));
  while(!emela_fiber_done(f)) {{ assert(emela_scheduler_run(1)); }}emela_fiber_release(f);
  if(!(i%2)) {{ assert(emela_export_read_3(result)==i);emela_embedded_release(result.payload.any_box); }}
  assert(test_heap_empty());
}} }}
"#,
            ty = TypeId::ANY.index()
        ),
    );
}

#[test]
fn opaque_board_handles_keep_their_pointer_identity_without_emela_ownership() {
    let mut core = emela_core::Module::new();
    let id = core.add_enum(EnumDef {
        name: "BoardToken".into(),
        params: 0,
        variants: vec![],
        error: false,
        external: true,
    });
    let ty = core.enum_type(id, vec![]);
    let output = compile_flow(&echo_flow(core, ty), &options().with_heap_bytes(0)).unwrap();
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask task;static void*saved;
uint32_t emela_board_event_lock(void) {{ return 0; }}
void emela_board_event_unlock(uint32_t p) {{ (void)p; }}
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) {{ (void)k;(void)m;(void)n;abort(); }}
void board_value_start(EmelaTask t,void*value) {{ task=t;saved=value; }}
static void produce(void*p,void**out) {{ *out=p; }}
int main(void) {{ int token=42;for(int i=0;i<1000;i++) {{
  void*result=NULL;EmelaFiber f=emela_export_echo_1_start(&token,&result);while(emela_scheduler_run(1)) {{}}
  assert(emela_task_complete_value_{ty}(task,produce,saved,NULL));
  while(!emela_fiber_done(f)) {{ assert(emela_scheduler_run(1)); }}
  emela_fiber_release(f);assert(result==&token);
}} }}
"#,
            ty = ty.index()
        ),
    );
}
