use super::*;

fn output() -> Output {
    let core = support::try_lower(
        &[(
            "main.emel",
            include_str!("../../examples/support/external_task_values.emel"),
        )],
        Roots::Exported,
    )
    .unwrap();
    let mut output = compile(
        &core,
        &options()
            .with_string_capacity(64)
            .with_task_adapter("host_packet", "board_packet_start")
            .with_task_adapter("host_text", "board_text_start"),
    )
    .unwrap();
    add_heap_check(&mut output);
    output
}

pub(super) fn add_heap_check(output: &mut Output) {
    output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents
        .push_str(
            r#"
#ifdef EMELA_UNWIND_RUNTIME
void test_cancel(EmelaFiber f) { emela_fiber_get(f)->cancelled=1; }
#endif
int test_heap_empty(void) {
  if (!emela_heap_initialized) return 1;
  for (unsigned char *p=emela_heap_storage;p<emela_heap_end();) {
    emela_heap_block *b=(emela_heap_block *)(void*)p;
    if (!b->is_free) { return 0; }
    p+=b->size;
  }
  return 1;
}
"#,
        );
}

fn packet_ids(output: &Output) -> (usize, usize) {
    let header = &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.h")
        .unwrap()
        .contents;
    let ty: usize = header
        .lines()
        .find(|l| l.starts_with("void board_packet_start("))
        .unwrap()
        .split("emela_type_")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let release: usize = header
        .lines()
        .find(|l| {
            l.starts_with("void emela_embedded_release_value_")
                && l.contains(&format!("emela_type_{ty} *"))
        })
        .unwrap()
        .trim_start_matches("void emela_embedded_release_value_")
        .split('(')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    (ty, release)
}

#[test]
fn source_tasks_own_packet_arguments_and_successes_through_scope_deferred_cancel_and_null_results()
{
    let output = output();
    let (ty, release) = packet_ids(&output);
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
void test_cancel(EmelaFiber f);
int test_heap_empty(void);
static EmelaTask pending;
static emela_type_{ty} board_value;
static int produced,released,locked,isr;
uint32_t emela_board_event_lock(void) {{ int old=locked;locked=1;return old; }}
void emela_board_event_unlock(uint32_t p) {{ assert(locked);locked=(int)p; }}
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) {{ fprintf(stderr,"%d:%.*s\n",k,n,m);abort(); }}
void board_packet_start(EmelaTask t,emela_type_{ty} value) {{
  assert(!locked&&!isr);pending=t;board_value=value;emela_embedded_retain_value_{release}(&board_value);
}}
void board_text_start(EmelaTask t,EmelaString value) {{ (void)t;(void)value;abort(); }}
static void produce(void*p,emela_type_{ty}*value) {{ assert(!locked&&!isr);produced++;*value=*(emela_type_{ty}*)p; }}
static void other(void*p,EmelaString*out) {{ (void)p;(void)out;abort(); }}
static void release(void*p) {{ assert(!locked&&!isr);released++;EmelaString text=((emela_type_{ty}*)p)->payload.v0.f0.f0;assert(text.length==2&&!memcmp(text.data,"42",2));emela_embedded_release_value_{release}(p);memset(p,0,sizeof(emela_type_{ty})); }}
static void drain(void) {{ while(emela_scheduler_run(1)) {{}} }}
int main(void) {{
  for(int i=0;i<1000;i++) {{
    for(int route=0;route<3;route++) {{
      emela_type_{ty} input=emela_export_seed_4(42),result={{0}};produced=released=0;
      int cancel=i%5==1,discard=i%3==0;
      EmelaFiber f;
      if(route==0) f=emela_export_exchange_0_start(input,discard?NULL:&result);
      else if(route==1) f=emela_export_scoped_2_start(input,discard?NULL:&result);
      else f=emela_export_deferred_3_start(input,discard?NULL:&result);
      emela_embedded_release_value_{release}(&input);drain();
      assert(!emela_fiber_done(f)&&board_value.tag==0);
      isr=1;assert(!emela_task_complete_int(pending,42));
      assert(!emela_task_complete_value_{string}(pending,other,NULL,release));
      assert(emela_task_complete_value_{ty}(pending,produce,&board_value,release));
      assert(!emela_task_complete_value_{ty}(pending,produce,&board_value,release));
      assert(!produced&&!released);isr=0;
      if(cancel) {{ test_cancel(f); }}
      drain();assert(emela_fiber_done(f));emela_fiber_release(f);
      assert(produced==!cancel&&released==1);
      if(!discard&&!cancel) {{
        EmelaString text=result.payload.v0.f0.f0;
        assert(result.tag==0&&text.owner&&text.length==2&&!memcmp(text.data,"42",2));
        assert(result.payload.v0.f0.f1.tag==0&&result.payload.v0.f0.f1.payload.v0.f0.owner);
        emela_embedded_release_value_{release}(&result);
      }}
      assert(test_heap_empty());
    }}
  }}
}}
"#,
            string = TypeId::STRING.index()
        ),
    );
}

#[test]
fn string_successes_snapshot_nul_bytes_before_the_context_is_released() {
    let output = output();
    let (ty, _) = packet_ids(&output);
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
void test_cancel(EmelaFiber f);int test_heap_empty(void);
static EmelaTask pending;static char board_bytes[3];static int produced,released;
uint32_t emela_board_event_lock(void) {{ return 0; }}
void emela_board_event_unlock(uint32_t p) {{ (void)p; }}
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) {{ fprintf(stderr,"%d:%.*s\n",k,n,m);abort(); }}
void board_packet_start(EmelaTask t,emela_type_{ty} v) {{ (void)t;(void)v;abort(); }}
void board_text_start(EmelaTask t,EmelaString value) {{ assert(value.length==3);pending=t;memcpy(board_bytes,value.data,3); }}
static void produce(void*p,EmelaString*out) {{ produced++;*out=(EmelaString){{.data=p,.length=3}}; }}
static void release(void*p) {{ released++;memset(p,'x',3); }}
static void drain(void) {{ while(emela_scheduler_run(1)) {{}} }}
int main(void) {{ for(int i=0;i<1000;i++) {{
  char original[]={{'a','\0','b'}};EmelaString result={{0}};produced=released=0;
  int cancel=i%5==1,discard=i%3==0;
  EmelaFiber f=emela_export_echoed_1_start((EmelaString){{.data=original,.length=3}},discard?NULL:&result);
  memset(original,'z',3);drain();assert(!memcmp(board_bytes,"a\0b",3));
  assert(emela_task_complete_value_{string}(pending,produce,board_bytes,release));
  if(cancel) {{ test_cancel(f); }}
      drain();assert(emela_fiber_done(f));emela_fiber_release(f);
  assert(produced==!cancel&&released==1);
  if(!discard&&!cancel) {{ assert(result.owner&&result.length==3&&!memcmp(result.data,"a\0b",3));emela_embedded_release(result.owner); }}
  assert(test_heap_empty());
}} }}
"#,
            string = TypeId::STRING.index()
        ),
    );
}
