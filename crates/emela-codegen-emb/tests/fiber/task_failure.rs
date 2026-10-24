use super::*;

#[path = "../../examples/support/external_task_failure_flow.rs"]
mod fixture;

fn output() -> Output {
    let mut output = compile_flow(
        &fixture::lower(),
        &options()
            .with_heap_bytes(4096)
            .with_string_capacity(256)
            .with_frame_bytes(1536)
            .with_task_adapter("host_request", "board_request_start"),
    )
    .unwrap();
    notes::add_heap_check(&mut output);
    output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents
        .push_str("void test_cancel(EmelaFiber f) { emela_fiber_get(f)->cancelled = 1; }\n");
    output
}

fn error_ids(output: &Output) -> (usize, usize) {
    let header = &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.h")
        .unwrap()
        .contents;
    let field = header
        .lines()
        .find(|l| {
            l.trim_start().starts_with("emela_type_")
                && l.trim_end().ends_with(';')
                && l.contains(" e")
        })
        .unwrap();
    let mut fields = field.split_whitespace();
    let ty = fields
        .next()
        .unwrap()
        .trim_start_matches("emela_type_")
        .parse()
        .unwrap();
    let tag: usize = fields
        .next()
        .unwrap()
        .trim_start_matches('e')
        .trim_end_matches(';')
        .parse()
        .unwrap();
    (ty, tag)
}

fn scalar_flow(indirect: bool) -> Module {
    let mut core = emela_core::Module::new();
    let error = core.add_enum(EnumDef {
        name: "Boom".into(),
        params: 0,
        variants: vec![VariantDef::positional("Boom", vec![TypeId::INT])],
        error: true,
        external: false,
    });
    let error_ty = core.enum_type(error, vec![]);
    let callback_ty = core.types.intern(Type::Fn(FnType {
        params: vec![],
        ret: TypeId::INT,
        fails: true,
        suspends: true,
        ev: false,
        sets: None,
    }));
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "request");
    b.set_suspends().set_fails().set_ret(TypeId::INT);
    let external = flow.add(b.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("host_request".into()),
        }],
    }));
    let mut b = FnBuilder::new(&flow.core, "work");
    b.set_exported().set_suspends().set_ret(TypeId::INT);
    let callee = if indirect {
        Callee::Indirect(b.assign("callback", callback_ty, Value::FnRef(external)))
    } else {
        Callee::Direct(external)
    };
    let caught = b.var("caught", TypeId::ANY);
    b.block(Some(caught), "caught", |b, label| {
        let value = b.assign(
            "value",
            TypeId::INT,
            Value::Call {
                callee,
                args: vec![],
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
    let value = b.assign(
        "n",
        TypeId::INT,
        Value::Field {
            ctor: CtorRef {
                enum_id: error,
                variant: 0,
            },
            base: typed,
            index: 0,
        },
    );
    b.term(Term::Return(Some(value.into())));
    flow.add(b.finish());
    flow
}

#[test]
fn heapless_external_failure_supports_direct_and_indirect_suspend_calls() {
    for indirect in [false, true] {
        let output = compile_flow(
            &scalar_flow(indirect),
            &options()
                .with_heap_bytes(0)
                .with_task_adapter("host_request", "board_request_start"),
        )
        .unwrap();
        let (ty, tag) = error_ids(&output);
        compile_and_run(
            output,
            &format!(
                r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask task;
static int produced,released;
uint32_t emela_board_event_lock(void) {{ return 0; }}
void emela_board_event_unlock(uint32_t p) {{ (void)p; }}
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) {{ (void)k;(void)m;(void)n;abort(); }}
void board_request_start(EmelaTask t) {{ task=t; }}
static void produce(void *p,EmelaError *e) {{ produced++;*e=(EmelaError){{.tag={tag}+1,.payload.e{tag}=(emela_type_{ty}){{.tag=0,.payload.v0.f0=*(int32_t*)p}}}}; }}
static void release(void*p) {{ released++;*(int32_t*)p=0; }}
int main(void) {{
  for(int i=0;i<1000;i++) {{
    int32_t result=-1,n=42;produced=released=0;EmelaFiber f=emela_export_work_1_start(&result);
    while(emela_scheduler_run(1)) {{}}
    assert(emela_task_fail(task,produce,&n,i%2?NULL:release));assert(!produced&&!released&&n==42);
    while(!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result==42&&produced==1&&released==!(i%2)&&n==(i%2?42:0));emela_fiber_release(f);
  }}
}}
"#
            ),
        );
    }
}

#[test]
fn external_failure_queue_exhaustion_unlocks_without_running_callbacks() {
    let output = compile_flow(
        &scalar_flow(false),
        &options()
            .with_heap_bytes(0)
            .with_event_capacity(1)
            .with_task_adapter("host_request", "board_request_start"),
    )
    .unwrap();
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask tasks[2];
static int count,locked,produced,released;
uint32_t emela_board_event_lock(void) { int old=locked;locked=1;return old; }
void emela_board_event_unlock(uint32_t p) { assert(locked);locked=(int)p; }
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) {
  (void)m;(void)n;exit(k==EMELA_DEFECT_HEAP_EXHAUSTED&&!locked&&!produced&&!released?0:2);
}

void board_request_start(EmelaTask t) { assert(count<2);tasks[count++]=t; }
static void produce(void*p,EmelaError*e) { (void)p;(void)e;produced++; }
static void release(void*p) { (void)p;released++; }
int main(void) {
  (void)emela_export_work_1_start(NULL);(void)emela_export_work_1_start(NULL);
  while(emela_scheduler_run(1)) {}assert(count==2);
  assert(emela_task_fail(tasks[0],produce,NULL,release));
  (void)emela_task_fail(tasks[1],produce,NULL,release);return 1;
}
"#,
    );
}

#[test]
fn external_failure_rejects_empty_unknown_errors_and_scheduler_callback_reentry() {
    for mode in [0, 1, 2, 3] {
        let output = compile_flow(
            &scalar_flow(false),
            &options()
                .with_heap_bytes(0)
                .with_task_adapter("host_request", "board_request_start"),
        )
        .unwrap();
        let (ty, tag) = error_ids(&output);
        compile_and_run(
            output,
            &format!(
                r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask task;
static int locked,produced;
uint32_t emela_board_event_lock(void) {{ int old=locked;locked=1;return old; }}
void emela_board_event_unlock(uint32_t p) {{ assert(locked);locked=(int)p; }}
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) {{
  (void)m;(void)n;exit(k==EMELA_DEFECT_ASSERT&&!locked&&produced==1?0:2);
}}
void board_request_start(EmelaTask t) {{ task=t; }}
static void produce(void*p,EmelaError*e) {{ (void)p;produced++;
  if({mode}==2) (void)emela_scheduler_run(1);
  e->tag={mode}==0?0:UINT32_MAX;
  if({mode}==3) *e=(EmelaError){{.tag={tag}+1,.payload.e{tag}=(emela_type_{ty}){{.tag=99}}}};
}}
int main(void) {{
  (void)emela_export_work_1_start(NULL);while(emela_scheduler_run(1)) {{}}
  assert(emela_task_fail(task,produce,NULL,NULL));
  while(emela_scheduler_run(1)) {{}}return 1;
}}
"#
            ),
        );
    }
}

#[test]
fn external_failures_own_borrowed_strings_and_lists_across_escape_attempt_scope_and_cancel() {
    let output = output();
    let (ty, tag) = error_ids(&output);
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
void test_cancel(EmelaFiber f);
static EmelaTask request, plain;
static int locked, isr, produced, released, marks;
typedef struct {{ char message[3]; void *parts; }} Context;
uint32_t emela_board_event_lock(void) {{ int old=locked; locked=1; return old; }}
void emela_board_event_unlock(uint32_t p) {{ assert(locked); locked=(int)p; }}
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) {{ fprintf(stderr,"%d:%.*s\n",k,n,m); abort(); }}
void board_request_start(EmelaTask t,int32_t n) {{ (void)n;request=t; }}
void board_wait_start(EmelaTask t,int32_t n) {{ (void)n;plain=t; }}
void mark(int32_t n) {{ assert(n==1 && !isr && !locked); marks++; }}
static void produce(void *p,EmelaError *error) {{
  Context *c=p;assert(!isr && !locked);produced++;
  *error=(EmelaError){{.tag={tag}+1,.payload.e{tag}=(emela_type_{ty}){{.tag=0,.payload.v0={{
    .f0={{.data=c->message,.length=3}},.f1=c->parts
  }}}}}};
}}
static void release(void *p) {{ Context*c=p;assert(!isr && !locked);released++;memset(c->message,'x',3);emela_embedded_release(c->parts);c->parts=NULL; }}
static void drain(void) {{ while(emela_scheduler_run(1)) {{}} }}
static void fail(Context *c) {{ isr=1;assert(emela_task_fail(request,produce,c,release));
  assert(!emela_task_fail(request,produce,c,release));assert(!emela_task_complete_int(request,99));
  assert(!produced && !released);isr=0; }}
int main(void) {{
  for(int i=0;i<1000;i++) {{
    Context c={{.message={{'a','\0','b'}},.parts=emela_export_parts_4(42)}};
    produced=released=marks=0;int32_t result=-1;
    EmelaFiber f=emela_export_caught_0_start(i,&result);drain();fail(&c);drain();
    assert(emela_fiber_done(f)&&result==5&&produced==1&&released==1&&marks==1);
    emela_fiber_release(f);assert(test_heap_empty());

    c=(Context){{.message={{'a','b','c'}},.parts=emela_export_parts_4(42)}};produced=released=0;
    EmelaString text={{0}};f=emela_export_attempted_1_start(i,i%2?NULL:&text);drain();fail(&c);drain();
    assert(emela_fiber_done(f)&&produced==1&&released==1);emela_fiber_release(f);
    if(!(i%2)) {{ const char expected[]="error: Boom(message: \"abc\", parts: [\"42\"])";
      assert(text.length==strlen(expected)&&!memcmp(text.data,expected,text.length));emela_embedded_release(text.owner); }}
    assert(test_heap_empty());

    c=(Context){{.message={{'a','b','c'}},.parts=emela_export_parts_4(42)}};produced=released=0;result=-1;
    f=emela_export_scoped_2_start(i,&result);drain();fail(&c);drain();
    assert(emela_fiber_done(f)&&result==3&&produced==1&&released==1);emela_fiber_release(f);assert(test_heap_empty());

    c=(Context){{.message={{'a','b','c'}},.parts=emela_export_parts_4(42)}};produced=released=marks=0;result=-1;
    f=emela_export_caught_0_start(i,&result);drain();fail(&c);test_cancel(f);
    (void)emela_scheduler_run(0);assert(!produced&&!released);
    assert(!test_heap_empty());drain();assert(emela_fiber_done(f)&&!produced&&released==1&&marks==1&&result==-1);emela_fiber_release(f);assert(test_heap_empty());
    assert(!emela_task_fail(request,produce,&c,release));

    produced=released=marks=0;result=-1;f=emela_export_caught_0_start(i,&result);drain();test_cancel(f);drain();
    assert(emela_fiber_done(f)&&!emela_task_fail(request,produce,&c,release));emela_fiber_release(f);assert(test_heap_empty());

    produced=released=marks=0;result=-1;f=emela_export_caught_0_start(i,&result);drain();
    assert(emela_task_complete_int(request,42));drain();assert(emela_fiber_done(f)&&result==42&&marks==1);emela_fiber_release(f);assert(test_heap_empty());

    f=emela_export_plain_3_start(i,&result);drain();assert(!emela_task_fail(plain,produce,&c,release));assert(emela_task_complete_int(plain,42));drain();assert(result==42);emela_fiber_release(f);
    assert(!emela_task_fail((EmelaTask){{UINT32_MAX,0}},produce,&c,release));
    assert(!emela_task_fail(request,NULL,&c,release));assert(!produced&&!released&&test_heap_empty());
  }}
}}
"#
        ),
    );
}

#[test]
fn external_failure_payload_survives_suspending_cleanup_and_later_interruption() {
    let output = output();
    let (ty, tag) = error_ids(&output);
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
void test_cancel(EmelaFiber f);
static EmelaTask request,cleanup;
static int produced,released,marks;
typedef struct {{ char message[3];void *parts; }} Context;
uint32_t emela_board_event_lock(void) {{ return 0; }}
void emela_board_event_unlock(uint32_t p) {{ (void)p; }}
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) {{ fprintf(stderr,"%d:%.*s\n",k,n,m);abort(); }}
void board_request_start(EmelaTask t,int32_t n) {{ (void)n;request=t; }}
void board_wait_start(EmelaTask t,int32_t n) {{ assert(n==99);cleanup=t; }}
void mark(int32_t n) {{ assert(n==2);marks++; }}
static void produce(void*p,EmelaError*e) {{ Context*c=p;produced++;
  *e=(EmelaError){{.tag={tag}+1,.payload.e{tag}=(emela_type_{ty}){{.tag=0,.payload.v0={{
    .f0={{.data=c->message,.length=3}},.f1=c->parts
  }}}}}};
}}
static void release(void*p) {{ Context*c=p;released++;memset(c->message,'x',3);emela_embedded_release(c->parts); }}
static void drain(void) {{ while(emela_scheduler_run(1)) {{}} }}
int main(void) {{
  for(int i=0;i<1000;i++) {{
    Context c={{.message={{'a','\0','b'}},.parts=emela_export_parts_4(42)}};
    int32_t result=-1;produced=released=marks=0;
    EmelaFiber f=emela_export_cleaned_5_start(i,i%3?&result:NULL);drain();
    assert(emela_task_fail(request,produce,&c,release));drain();
    assert(produced==1&&released==1&&!marks&&!emela_fiber_done(f)&&!test_heap_empty());
    if(i%2) {{ test_cancel(f);drain();assert(!emela_fiber_done(f)); }}
    assert(emela_task_complete_int(cleanup,0));drain();
    assert(emela_fiber_done(f)&&marks==1);emela_fiber_release(f);
    assert(result==((i%3&&!(i%2))?5:-1)&&test_heap_empty());
  }}
}}
"#
        ),
    );
}

#[test]
fn abandoned_failure_events_release_contexts_with_unwind_but_without_resume_runtime() {
    let mut flow = scalar_flow(false);
    let (_, work) = flow
        .functions
        .iter_mut()
        .find(|(_, f)| f.name == "work")
        .unwrap();
    let body = work.body.unwrap();
    let cleanup = work.seqs.alloc(emela_lir::Seq {
        stmts: vec![],
        term: Term::Yield(None),
    });
    let protect = work.stmts.alloc(emela_lir::Stmt::Protect {
        dst: None,
        body,
        cleanup,
    });
    work.body = Some(work.seqs.alloc(emela_lir::Seq {
        stmts: vec![protect],
        term: Term::Unreachable,
    }));
    let mut output = compile_flow(
        &flow,
        &options()
            .with_heap_bytes(0)
            .with_task_adapter("host_request", "board_request_start"),
    )
    .unwrap();
    let source = &mut output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents;
    assert!(source.contains("#define EMELA_UNWIND_RUNTIME 1"));
    assert!(!source.contains("#define EMELA_RESUME_RUNTIME 1"));
    source.push_str("void test_cancel(EmelaFiber f) { emela_fiber_get(f)->cancelled=1; }\n");
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
void test_cancel(EmelaFiber f);
static EmelaTask task;
static int produced,released;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) { (void)k;(void)m;(void)n;abort(); }
void board_request_start(EmelaTask t) { task=t; }
static void produce(void*p,EmelaError*e) { (void)p;(void)e;produced++; }
static void release(void*p) { (void)p;released++; }
int main(void) {
  for(int i=0;i<1000;i++) {
    produced=released=0;EmelaFiber f=emela_export_work_1_start(NULL);
    while(emela_scheduler_run(1)) {}assert(emela_task_fail(task,produce,NULL,release));
    test_cancel(f);(void)emela_scheduler_run(0);assert(!produced&&!released);
    while(emela_scheduler_run(1)) {}assert(emela_fiber_done(f)&&!produced&&released==1);
    emela_fiber_release(f);
  }
}
"#,
    );
}
