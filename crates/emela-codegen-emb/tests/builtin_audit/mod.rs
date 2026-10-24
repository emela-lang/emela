use super::*;

mod retire;

fn discarded_builtin(op: Builtin, input: TypeId) -> Module {
    let mut flow = Module::new(emela_core::Module::new());
    let mut b = FnBuilder::new(&flow.core, "discard");
    b.set_exported();
    let input = b.param("input", input);
    b.let_(
        None,
        Value::Builtin {
            op,
            args: vec![input.into()],
        },
    );
    b.term(Term::Return(None));
    flow.add(b.finish());
    flow
}

#[test]
fn unreachable_discarded_results_do_not_introduce_heap_requirements() {
    let mut flow = Module::new(emela_core::Module::new());
    let mut b = FnBuilder::new(&flow.core, "answer");
    b.set_exported().set_ret(TypeId::INT);
    b.term(Term::Return(Some(Const::Int(42).into())));
    let mut function = b.finish();
    function.stmts.alloc(emela_lir::Stmt::Let {
        dst: None,
        value: Value::Builtin {
            op: Builtin::StringChars,
            args: vec![Const::String("unreachable".into()).into()],
        },
        on_defect: None,
        on_suspend: None,
    });
    flow.add(function);
    emela_lir::verify(&flow).expect("orphan statement arenas are permitted by the Flow verifier");
    let output = compile_flow(&flow, &Options::default())
        .expect("unreachable discarded builtin must not create heap-backed locals");
    compile_and_run(
        output,
        "#include <assert.h>\n#include \"emela_embedded.h\"\nint main(void) { assert(emela_export_answer_0() == 42); }\n",
    );
}

#[test]
fn discarded_string_producers_share_scratch_and_preserve_numeric_bounds() {
    let mut flow = Module::new(emela_core::Module::new());
    for (name, input, op) in [
        ("integers", TypeId::INT, Builtin::IntToString),
        ("strings", TypeId::STRING, Builtin::StringAppend),
    ] {
        let mut b = FnBuilder::new(&flow.core, name);
        b.set_exported();
        let input = b.param("input", input);
        for _ in 0..13 {
            let mut args = vec![input.into()];
            if op == Builtin::StringAppend {
                args.push(Const::String("!".into()).into());
            }
            b.let_(None, Value::Builtin { op, args });
        }
        b.term(Term::Return(None));
        flow.add(b.finish());
    }
    let output = compile_flow(&flow, &Options::default()).unwrap();
    let source = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.c")
        .unwrap()
        .contents;
    assert_eq!(source.matches("char emela_string_buffer_").count(), 2);
    assert!(source.contains("char emela_string_buffer_0[11];"));
    assert!(source.contains("char emela_string_buffer_0[256];"));
    assert!(!source.contains("emela_heap_storage"));
    compile_and_run(
        output,
        r#"
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) { (void)k;(void)m;(void)n;abort(); }
int main(void) { for (int i=0;i<1000;i++) {
  emela_export_integers_0(INT32_MIN);
  emela_export_strings_1((EmelaString){.data="abc",.length=3});
} }
"#,
    );
}

#[test]
fn discarded_float_rounding_preserves_the_out_of_range_defect() {
    let output = compile_flow(
        &discarded_builtin(Builtin::FloatFloor, TypeId::FLOAT),
        &Options::default(),
    )
    .expect("a discarded nonvoid builtin is valid Flow");
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <math.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) { (void)m;(void)n;exit(k==EMELA_DEFECT_FLOAT_TO_INT?0:2); }
int main(void) { emela_export_discard_0(NAN);return 1; }
"#,
    );
}

#[test]
fn discarded_string_chars_still_builds_and_releases_its_owned_list() {
    let mut flow = discarded_builtin(Builtin::StringChars, TypeId::STRING);
    let (_, function) = flow.functions.iter_mut().next().unwrap();
    let statement = function.seqs[function.body.unwrap()].stmts[0];
    for _ in 0..12 {
        let duplicate = function.stmts.alloc(function.stmts[statement].clone());
        function.seqs[function.body.unwrap()].stmts.push(duplicate);
    }
    let mut output = compile_flow(&flow, &Options::default().with_heap_bytes(512))
        .expect("discarded list result is valid Flow");
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
  for (unsigned char*p=emela_heap_storage;p<emela_heap_end();) {
    emela_heap_block*b=(emela_heap_block*)(void*)p;
    if(!b->is_free) { return 0; }p+=b->size;
  }
  return 1;
}
"#,
        );
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) { (void)k;(void)m;(void)n;abort(); }
int main(void) { for(int i=0;i<1000;i++) { char bytes[]={'a','\0','b'};emela_export_discard_0((EmelaString){.data=bytes,.length=3});assert(test_heap_empty()); } }
"#,
    );
}

#[test]
fn discarded_runtime_show_string_keeps_its_capacity_check() {
    let mut flow = Module::new(emela_core::Module::new());
    let mut b = FnBuilder::new(&flow.core, "discard");
    b.set_exported();
    let input = b.param("input", TypeId::STRING);
    b.let_(
        None,
        Value::Runtime {
            op: RuntimeOp::ShowString,
            args: vec![input.into()],
        },
    );
    b.term(Term::Return(None));
    flow.add(b.finish());
    let output = compile_flow(&flow, &Options::default().with_string_capacity(3))
        .expect("discarded ShowString is valid Flow");
    compile_and_run(
        output,
        r#"
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) { (void)m;(void)n;exit(k==EMELA_DEFECT_STRING_CAPACITY?0:2); }
int main(void) { emela_export_discard_0((EmelaString){.data="abc",.length=3});return 1; }
"#,
    );
}

#[test]
fn discarded_option_builtins_infer_their_result_layout_without_a_source_option_type() {
    for (op, ty, source) in [
        (
            Builtin::IntParse,
            TypeId::STRING,
            "(EmelaString){.data=\"42\",.length=2}",
        ),
        (
            Builtin::FloatParse,
            TypeId::STRING,
            "(EmelaString){.data=\"0.5\",.length=3}",
        ),
        (
            Builtin::StringFromCodePoint,
            TypeId::INT,
            "INT32_C(0x1f600)",
        ),
    ] {
        let output = compile_flow(&discarded_builtin(op, ty), &Options::default()).unwrap();
        compile_and_run(
            output,
            &format!(
                r#"
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) {{ (void)k;(void)m;(void)n;abort(); }}
int main(void) {{ for(int i=0;i<1000;i++) emela_export_discard_0({source}); }}
"#
            ),
        );
    }
}

#[test]
fn discarded_owned_results_do_not_survive_a_following_suspension() {
    let mut flow = Module::new(emela_core::Module::new());
    let mut b = FnBuilder::new(&flow.core, "wait");
    b.set_suspends();
    let wait = flow.add(b.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("host_wait".into()),
        }],
    }));
    let mut b = FnBuilder::new(&flow.core, "work");
    b.set_suspends().set_exported();
    let input = b.param("input", TypeId::STRING);
    b.let_(
        None,
        Value::Builtin {
            op: Builtin::StringChars,
            args: vec![input.into()],
        },
    );
    b.let_(
        None,
        Value::Builtin {
            op: Builtin::StringAppend,
            args: vec![input.into(), Const::String("!".into()).into()],
        },
    );
    b.let_(
        None,
        Value::Call {
            callee: Callee::Direct(wait),
            args: vec![],
            suspends: true,
            on_fail: None,
        },
    );
    b.term(Term::Return(None));
    flow.add(b.finish());
    let mut output = compile_flow(
        &flow,
        &Options::default()
            .with_heap_bytes(1024)
            .with_frame_bytes(1024)
            .with_task_adapter("host_wait", "board_wait_start"),
    )
    .unwrap();
    output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents
        .push_str(
            r#"
int test_heap_used_blocks(void) {
  int used=0;if(!emela_heap_initialized)return 0;
  for(unsigned char*p=emela_heap_storage;p<emela_heap_end();) {
    emela_heap_block*b=(emela_heap_block*)(void*)p;
    used+=!b->is_free;p+=b->size;
  }return used;
}
"#,
        );
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_used_blocks(void);static EmelaTask task;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t) { task=t; }
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) { (void)k;(void)m;(void)n;abort(); }
int main(void) { for(int i=0;i<1000;i++) {
  EmelaFiber f=emela_export_work_1_start((EmelaString){.data="abc",.length=3});while(emela_scheduler_run(1)) {}
  assert(!emela_fiber_done(f)&&test_heap_used_blocks()==1);assert(emela_task_complete_unit(task));
  while(!emela_fiber_done(f)) { assert(emela_scheduler_run(1)); }emela_fiber_release(f);assert(!test_heap_used_blocks());
} }
"#,
    );
}
