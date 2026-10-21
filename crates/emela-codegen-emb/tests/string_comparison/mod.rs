use super::*;

const SOURCE: &str = include_str!("../../examples/support/string_comparison.emel");

fn c_source(output: &Output) -> &str {
    &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents
}

const CHECKS: &str = r#"
static void check(EmelaString a, EmelaString b, int order) {
  assert(emela_export_eq_0(a,b) == (order == 0));
  assert(emela_export_ne_1(a,b) == (order != 0));
  assert(emela_export_lt_2(a,b) == (order < 0));
  assert(emela_export_le_3(a,b) == (order <= 0));
  assert(emela_export_gt_4(a,b) == (order > 0));
  assert(emela_export_ge_5(a,b) == (order >= 0));
}
static void boundaries(void) {
  const char a[] = {'a',0,'b'}, b[] = {'a',0,'c'}, same[] = {'a',0,'b'};
  check((EmelaString){NULL,0,NULL},(EmelaString){NULL,0,NULL},0);
  check((EmelaString){NULL,0,NULL},(EmelaString){a,3,NULL},-1);
  check((EmelaString){a,3,NULL},(EmelaString){NULL,0,NULL},1);
  check((EmelaString){a,3,NULL},(EmelaString){same,3,NULL},0);
  check((EmelaString){a,3,NULL},(EmelaString){b,3,NULL},-1);
  check((EmelaString){a,3,NULL},(EmelaString){a,1,NULL},1);
  check((EmelaString){"\xee\x80\x80",3,NULL},(EmelaString){"\xf0\x90\x80\x80",4,NULL},-1);
  check((EmelaString){"\x7f",1,NULL},(EmelaString){"\xc2\x80",2,NULL},-1);
  check((EmelaString){"e\xcc\x81",3,NULL},(EmelaString){"\xc3\xa9",2,NULL},-1);
  static char long_a[UINT16_MAX], long_b[UINT16_MAX];
  memset(long_a,'a',sizeof(long_a)); memset(long_b,'a',sizeof(long_b));
  check((EmelaString){long_a,UINT16_MAX,NULL},(EmelaString){long_b,UINT16_MAX,NULL},0);
  long_b[UINT16_MAX-1]='b';
  check((EmelaString){long_a,UINT16_MAX,NULL},(EmelaString){long_b,UINT16_MAX,NULL},-1);
  assert(emela_export_repeated_6((EmelaString){a,3,NULL},(EmelaString){same,3,NULL},1000));
  assert(!emela_export_repeated_6((EmelaString){a,3,NULL},(EmelaString){b,3,NULL},1001));
}
"#;

const PRELUDE: &str = r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m); abort(); }
"#;

#[test]
fn source_comparisons_are_heapless_and_allow_isr_and_constant_space_tail_loops() {
    let core = support::try_lower(&[("main.emel", SOURCE)], Roots::Exported).unwrap();
    let output = compile(&core, &Options::default().with_isr_entry("lt")).unwrap();
    let source = c_source(&output);
    assert!(source.contains("emela_emb_string_compare"));
    assert!(!source.contains("emela_heap_storage"));
    assert!(!source.contains("char emela_string_buffer"));
    compile_and_run(
        output,
        &format!("{PRELUDE}\n{CHECKS}\nint main(void) {{ boundaries(); return 0; }}\n"),
    );
}

#[test]
fn all_constant_comparisons_match_runtime_order_without_emitting_the_helper() {
    let core = emela_core::Module::new();
    let mut flow = Module::new(core);
    let pairs = [
        ("", ""),
        ("a\0b", "a\0c"),
        ("ab", "a"),
        ("\u{e000}", "\u{10000}"),
        ("é", "e\u{301}"),
    ];
    let ops = [
        PrimOp::Eq,
        PrimOp::Ne,
        PrimOp::Lt,
        PrimOp::Le,
        PrimOp::Gt,
        PrimOp::Ge,
    ];
    let mut checks = String::new();
    for (a, b) in pairs {
        for op in ops {
            let index = flow.functions.len();
            let mut builder = FnBuilder::new(&flow.core, &format!("compare_{index}"));
            builder.set_ret(TypeId::BOOL).set_exported().set_constant();
            let value = builder.assign(
                "result",
                TypeId::BOOL,
                Value::Prim {
                    op,
                    ty: PrimTy::String,
                    args: vec![
                        Const::String(a.into()).into(),
                        Const::String(b.into()).into(),
                    ],
                },
            );
            builder.term(Term::Return(Some(value.into())));
            flow.add(builder.finish());
            let expected = match op {
                PrimOp::Eq => a == b,
                PrimOp::Ne => a != b,
                PrimOp::Lt => a < b,
                PrimOp::Le => a <= b,
                PrimOp::Gt => a > b,
                PrimOp::Ge => a >= b,
                _ => unreachable!(),
            };
            checks.push_str(&format!(
                "assert(emela_export_compare_{index}_{index}()=={});\n",
                u8::from(expected)
            ));
        }
    }
    let output = compile_flow(&flow, &Options::default()).unwrap();
    assert!(!c_source(&output).contains("emela_emb_string_compare"));
    compile_and_run(
        output,
        &format!("{PRELUDE}\nint main(void) {{ {checks} return 0; }}\n"),
    );
}

#[test]
fn owned_string_views_compare_without_allocating_or_changing_ownership() {
    let source = format!(
        "{SOURCE}\npub fn store(text: String) -> List[String] {{ [text] }}\npub fn read(values: List[String]) -> String {{ values match {{ [head, ..rest] -> head\n [] -> \"\" }} }}\n"
    );
    let core = support::try_lower(&[("main.emel", &source)], Roots::Exported).unwrap();
    let mut output = compile(&core, &Options::default().with_heap_bytes(1024)).unwrap();
    output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents
        .push_str(
            r#"
size_t test_heap_used(void) {
  size_t used=0;
  if (!emela_heap_initialized) return 0;
  for(unsigned char *at=emela_heap_storage;at<emela_heap_end();) {
    emela_heap_block *block=(emela_heap_block *)(void *)at;
    if(!block->is_free) used+=block->size;
    at+=block->size;
  }
  return used;
}
"#,
        );
    compile_and_run(
        output,
        &format!(
            r#"{PRELUDE}
{CHECKS}
size_t test_heap_used(void);
int main(void) {{
  boundaries();
  for(unsigned i=0;i<1000;i++) {{
    char data[]={{'x','a',0,'b','y'}};
    void *list=emela_export_store_7((EmelaString){{data,5,NULL}});
    EmelaString view=emela_export_read_8(list);
    emela_embedded_release(list); memset(data,'z',sizeof(data));
    assert(view.owner); view.data++; view.length=3;
    size_t before=test_heap_used();
    check(view,(EmelaString){{"a\0b",3,NULL}},0);
    check(view,(EmelaString){{"a\0c",3,NULL}},-1);
    assert(test_heap_used()==before);
    emela_embedded_release(view.owner); assert(test_heap_used()==0);
  }}
  return 0;
}}
"#
        ),
    );
}

#[test]
fn suspend_source_comparisons_preserve_captured_bytes_and_release_on_completion_and_cancel() {
    let core = support::try_lower(
        &[(
            "main.emel",
            include_str!("../../examples/support/string_comparison_suspend.emel"),
        )],
        Roots::Exported,
    )
    .unwrap();
    let mut output = compile(
        &core,
        &Options::default()
            .with_heap_bytes(4096)
            .with_fiber_capacity(2)
            .with_frame_capacity(8)
            .with_frame_bytes(512)
            .with_task_capacity(1)
            .with_event_capacity(1)
            .with_deferred_capacity(0)
            .with_task_adapter("host_wait", "board_wait_start"),
    )
    .unwrap();
    assert!(c_source(&output).contains("emela_emb_string_compare"));
    assert!(!c_source(&output).contains("char emela_string_buffer"));
    output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents
        .push_str(
            r#"
int test_heap_empty(void) {
  if (!emela_heap_initialized) return 1;
  for(unsigned char *at=emela_heap_storage;at<emela_heap_end();) {
    emela_heap_block *block=(emela_heap_block *)(void *)at;
    if(!block->is_free) return 0;
    at+=block->size;
  }
  return 1;
}
void test_cancel(EmelaFiber h) { emela_fiber_get(h)->cancelled=1; }
"#,
        );
    compile_and_run(
        output,
        &format!(
            r#"{PRELUDE}
int test_heap_empty(void);
void test_cancel(EmelaFiber h);
static EmelaTask task;
uint32_t emela_board_event_lock(void) {{ return 0; }}
void emela_board_event_unlock(uint32_t n) {{ (void)n; }}
void board_wait_start(EmelaTask t,int32_t n) {{ assert(n==1); task=t; }}
static void drain(void) {{ while(emela_scheduler_run(1)) {{}} }}
int main(void) {{
  for(unsigned i=0;i<1000;i++) {{
    for(unsigned route=0;route<3;route++) {{
      char a[]={{'a',0,'b'}},b[]={{'a',0,'c'}}; int32_t result=-1;
      EmelaFiber f=emela_export_across_0_start((EmelaString){{a,3,NULL}},(EmelaString){{b,3,NULL}},route==1?NULL:&result);
      memset(a,'z',3); memset(b,'z',3); drain();
      if(route==2) {{ test_cancel(f); drain(); }}
      else {{ assert(emela_task_complete_int(task,1)); drain(); if(route==0) assert(result==1); }}
      emela_fiber_release(f); assert(test_heap_empty());
    }}
  }}
  return 0;
}}
"#
        ),
    );
}
