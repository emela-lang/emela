use super::*;

fn with_heap_check(mut output: Output) -> Output {
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
  for (unsigned char *p = emela_heap_storage; p < emela_heap_end(); ) {
    emela_heap_block *b = (emela_heap_block *)(void *)p;
    if (!b->is_free) return 0;
    p += b->size;
  }
  return 1;
}
"#,
        );
    output
}

#[test]
fn shared_cells_keep_string_updates_alive_across_capture_suspend_and_cancellation() {
    let core = support::try_lower(
        &[(
            "main.emel",
            include_str!("../../examples/support/shared_cell.emel"),
        )],
        Roots::Exported,
    )
    .unwrap();
    let mut output = with_heap_check(
        compile(
            &core,
            &Options::default()
                .with_heap_bytes(8192)
                .with_string_capacity(128)
                .with_fiber_capacity(8)
                .with_frame_capacity(24)
                .with_frame_bytes(1024)
                .with_task_capacity(2)
                .with_event_capacity(2)
                .with_deferred_capacity(0)
                .with_task_adapter("host_wait", "board_wait_start"),
        )
        .unwrap(),
    );
    output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents
        .push_str("void test_cancel(EmelaFiber h) { emela_fiber_get(h)->cancelled = 1; }\n");
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
void test_cancel(EmelaFiber h);
static EmelaTask tasks[4];
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n",k,n,m); abort(); }
void board_wait_start(EmelaTask t, int32_t n) { assert(n == 10 || n == 20 || n == 30); tasks[n / 10] = t; }
static void drain(void) { while (emela_scheduler_run(1)) {} }
int main(void) {
  for (unsigned i = 0; i < 1000; i++) {
    assert(emela_export_escaped_result_1(40, NULL) == 83);
    assert(test_heap_empty());
    char composite_buffer[128];
    EmelaString composite = emela_export_composite_4(42, composite_buffer, sizeof(composite_buffer));
    assert(composite.length == 4 && memcmp(composite.data,"4242",4)==0);
    emela_embedded_release(composite.owner); assert(test_heap_empty());
    EmelaString result = {0};
    EmelaFiber fiber = emela_export_shared_2_start(42, &result); drain();
    assert(emela_task_complete_int(tasks[1], 0)); drain();
    assert(emela_task_complete_int(tasks[2], 0)); drain();
    assert(result.length == 31 && memcmp(result.data, "42 child parent/42 child parent", 31) == 0);
    emela_fiber_release(fiber); emela_embedded_release(result.owner); assert(test_heap_empty());
    fiber = emela_export_shared_2_start(42, NULL); drain();
    assert(emela_task_complete_int(tasks[2], 0)); drain();
    assert(emela_task_complete_int(tasks[1], 0)); drain();
    emela_fiber_release(fiber); assert(test_heap_empty());
    for (int bad = 0; bad < 2; bad++) {
      fiber = emela_export_failed_3_start(42, bad, &result); drain();
      assert(emela_task_complete_int(tasks[3], 0)); drain();
      assert(result.length == 10 && memcmp(result.data,"42 failure",10)==0);
      emela_fiber_release(fiber); emela_embedded_release(result.owner); assert(test_heap_empty());
    }
    fiber = emela_export_failed_3_start(42, 1, NULL); drain();
    test_cancel(fiber); drain(); emela_fiber_release(fiber); assert(test_heap_empty());
    for (unsigned step = 0; step < 2; step++) {
      fiber = emela_export_shared_2_start(42, &result); drain();
      if (step) { assert(emela_task_complete_int(tasks[1], 0)); drain(); }
      test_cancel(fiber); drain(); emela_fiber_release(fiber); assert(test_heap_empty());
    }
  }
  return 0;
}
"#,
    );
}

#[test]
fn cycles_through_cell_any_boxes_are_reclaimed_without_losing_external_roots() {
    let mut core = emela_core::Module::new();
    let cell_ty = core.types.intern(Type::Cell(TypeId::ANY));
    let function = {
        let mut b = FnBuilder::new(&core, "run");
        b.set_ret(cell_ty).set_exported();
        let n = b.param("n", TypeId::INT);
        let initial = b.assign("initial", TypeId::ANY, Value::Coerce(n));
        let cell = b.assign(
            "cell",
            cell_ty,
            Value::Builtin {
                op: Builtin::CellNew,
                args: vec![initial.into()],
            },
        );
        let boxed = b.assign("boxed", TypeId::ANY, Value::Coerce(cell));
        b.let_(
            None,
            Value::Builtin {
                op: Builtin::CellSet,
                args: vec![cell.into(), boxed.into()],
            },
        );
        b.term(Term::Return(Some(cell.into())));
        b.finish()
    };
    let mut flow = Module::new(core);
    flow.add(function);
    let output =
        with_heap_check(compile_flow(&flow, &Options::default().with_heap_bytes(1024)).unwrap());
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m); abort(); }
int main(void) { for (unsigned i=0;i<1000;i++) { void *root = emela_export_run_0(42); assert(!test_heap_empty()); emela_embedded_retain(root); emela_embedded_release(root); assert(!test_heap_empty()); emela_embedded_release(root); assert(test_heap_empty()); } return 0; }
"#,
    );
}

#[test]
fn cells_cross_aggregate_list_any_function_abis_and_snapshot_borrowed_strings() {
    let mut core = emela_core::Module::new();
    let cell = core.types.intern(Type::Cell(TypeId::STRING));
    let cells = core.list(cell);
    let read = core
        .types
        .intern(Type::Fn(FnType::pure(vec![], TypeId::STRING)));
    let identity = core.types.intern(Type::Fn(FnType::pure(vec![cell], cell)));
    let tuple = core.tuple(vec![cell, TypeId::STRING, cells, read]);
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "identity");
    b.set_ret(cell);
    let value = b.param("value", cell);
    b.term(Term::Return(Some(value.into())));
    let identity_id = flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "make");
    b.set_exported().set_ret(tuple);
    let text = b.param("text", TypeId::STRING);
    let value = b.assign(
        "cell",
        cell,
        Value::Builtin {
            op: Builtin::CellNew,
            args: vec![text.into()],
        },
    );
    let saved = b.assign(
        "saved",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::CellGet,
            args: vec![value.into()],
        },
    );
    b.let_(
        None,
        Value::Builtin {
            op: Builtin::CellSet,
            args: vec![value.into(), saved.into()],
        },
    );
    let boxed = b.assign("boxed", TypeId::ANY, Value::Coerce(value));
    let unboxed = b.assign("unboxed", cell, Value::Coerce(boxed));
    let callback = b.assign("identity", identity, Value::FnRef(identity_id));
    let aliased = b.assign(
        "aliased",
        cell,
        Value::Call {
            callee: Callee::Indirect(callback),
            args: vec![unboxed.into()],
            suspends: false,
            on_fail: None,
        },
    );
    let list = b.assign(
        "cells",
        cells,
        Value::List {
            elems: vec![aliased.into()],
            tail: None,
        },
    );
    let head = b.assign("head", cell, Value::ListHead(list));
    let reader = b.var("reader", read);
    b.closure(reader, vec![], vec![head], false, |b| {
        let got = b.assign(
            "got",
            TypeId::STRING,
            Value::Builtin {
                op: Builtin::CellGet,
                args: vec![head.into()],
            },
        );
        b.term(Term::Return(Some(got.into())));
    });
    let result = b.assign(
        "result",
        tuple,
        Value::Tuple(vec![head.into(), saved.into(), list.into(), reader.into()]),
    );
    b.term(Term::Return(Some(result.into())));
    flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "replace");
    b.set_exported();
    let target = b.param("target", cell);
    let text = b.param("text", TypeId::STRING);
    b.let_(
        None,
        Value::Builtin {
            op: Builtin::CellSet,
            args: vec![target.into(), text.into()],
        },
    );
    b.term(Term::Return(None));
    flow.add(b.finish());
    let output =
        with_heap_check(compile_flow(&flow, &Options::default().with_heap_bytes(2048)).unwrap());
    let source = &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents;
    assert!(
        !source.contains("emela_heap_collect_cycles"),
        "String-only Cell graphs cannot cycle"
    );
    let header = &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.h")
        .unwrap()
        .contents;
    let release = header
        .lines()
        .find(|s| {
            s.starts_with("void emela_embedded_release_value_")
                && s.contains(&format!("emela_type_{} *", tuple.index()))
        })
        .unwrap()
        .split('(')
        .next()
        .unwrap()
        .strip_prefix("void ")
        .unwrap()
        .to_owned();
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) {{ fprintf(stderr,"%d: %.*s\n",k,n,m); abort(); }}
int main(void) {{ for(unsigned i=0;i<1000;i++) {{
  char input[] = {{'a',0,'b'}};
  emela_type_{} value = emela_export_make_1((EmelaString){{input,3,NULL}});
  memset(input,'x',3);
  assert(value.f1.length==3 && memcmp(value.f1.data,"a\0b",3)==0);
  char next[] = {{'c',0,'d','e'}};
  emela_export_replace_2(value.f0,(EmelaString){{next,4,NULL}});
  memset(next,'y',4);
  EmelaString got = value.f3.call(value.f3.env);
  assert(got.length==4 && memcmp(got.data,"c\0de",4)==0);
  assert(memcmp(value.f1.data,"a\0b",3)==0);
  {release}(&value);
  assert(got.length==4 && memcmp(got.data,"c\0de",4)==0);
  emela_embedded_release(got.owner); assert(test_heap_empty());
}} return 0; }}
"#,
            tuple.index()
        ),
    );
}

#[path = "../../examples/support/shared_cell_cycle.rs"]
mod cycle_fixture;

#[test]
fn cycles_through_captured_cells_preserve_live_callbacks_and_reclaim_the_last_root() {
    let output = with_heap_check(
        compile_flow(
            &cycle_fixture::flow(),
            &Options::default().with_heap_bytes(1024),
        )
        .unwrap(),
    );
    let header = &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.h")
        .unwrap()
        .contents;
    let result_ty = header
        .lines()
        .find(|s| s.contains(" emela_export_make_1("))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned();
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) {{ fprintf(stderr,"%d: %.*s\n",k,n,m); abort(); }}
int main(void) {{ for(unsigned i=0;i<1000;i++) {{
  {result_ty} callback=emela_export_make_1();
  assert(!test_heap_empty());
  assert(callback.call(callback.env)==42);
  emela_embedded_retain(callback.env);
  emela_embedded_release(callback.env);
  assert(callback.call(callback.env)==42);
  emela_embedded_release(callback.env);
  assert(test_heap_empty());
}} return 0; }}
"#
        ),
    );
}

#[test]
fn nested_cells_and_cells_in_enum_loop_values_keep_shared_identity() {
    let mut core = emela_core::Module::new();
    let cell = core.types.intern(Type::Cell(TypeId::INT));
    let nested = core.types.intern(Type::Cell(cell));
    let enum_id = core.add_enum(EnumDef {
        name: "BoxedCell".into(),
        params: 0,
        variants: vec![VariantDef::positional("Box", vec![nested])],
        error: false,
        external: false,
    });
    let boxed = core.enum_type(enum_id, vec![]);
    let mut b = FnBuilder::new(&core, "run");
    b.set_ret(TypeId::INT).set_exported();
    let initial = b.assign(
        "initial",
        cell,
        Value::Builtin {
            op: Builtin::CellNew,
            args: vec![Const::Int(0).into()],
        },
    );
    let outer = b.assign(
        "outer",
        nested,
        Value::Builtin {
            op: Builtin::CellNew,
            args: vec![initial.into()],
        },
    );
    let wrapped = b.assign(
        "wrapped",
        boxed,
        Value::Make {
            ctor: CtorRef {
                enum_id,
                variant: 0,
            },
            args: vec![outer.into()],
        },
    );
    let state = b.var("state", boxed);
    let index = b.var("index", TypeId::INT);
    let result = b.var("result", TypeId::INT);
    b.loop_(
        Some(result),
        vec![(state, wrapped.into()), (index, Const::Int(0).into())],
        |b| {
            let projected = b.assign(
                "projected",
                nested,
                Value::Field {
                    base: state,
                    ctor: CtorRef {
                        enum_id,
                        variant: 0,
                    },
                    index: 0,
                },
            );
            let inner = b.assign(
                "inner",
                cell,
                Value::Builtin {
                    op: Builtin::CellGet,
                    args: vec![projected.into()],
                },
            );
            let value = b.assign(
                "value",
                TypeId::INT,
                Value::Builtin {
                    op: Builtin::CellGet,
                    args: vec![inner.into()],
                },
            );
            let next = b.assign(
                "next",
                TypeId::INT,
                Value::Prim {
                    op: PrimOp::Add,
                    ty: PrimTy::I32,
                    args: vec![value.into(), Const::Int(1).into()],
                },
            );
            b.let_(
                None,
                Value::Builtin {
                    op: Builtin::CellSet,
                    args: vec![inner.into(), next.into()],
                },
            );
            b.let_(
                None,
                Value::Builtin {
                    op: Builtin::CellSet,
                    args: vec![projected.into(), inner.into()],
                },
            );
            let end = b.assign(
                "end",
                TypeId::BOOL,
                Value::Prim {
                    op: PrimOp::Eq,
                    ty: PrimTy::I32,
                    args: vec![next.into(), Const::Int(42).into()],
                },
            );
            let finished = b.var("finished", TypeId::INT);
            b.if_(
                Some(finished),
                end,
                |b| {
                    b.term(Term::Yield(Some(next.into())));
                },
                |b| {
                    b.term(Term::Recur(vec![state.into(), next.into()]));
                },
            );
            b.term(Term::Yield(Some(finished.into())));
        },
    );
    b.term(Term::Return(Some(result.into())));
    let function = b.finish();
    let mut flow = Module::new(core);
    flow.add(function);
    let output =
        with_heap_check(compile_flow(&flow, &Options::default().with_heap_bytes(1024)).unwrap());
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m); abort(); }
int main(void){for(unsigned i=0;i<1000;i++){assert(emela_export_run_0()==42);assert(test_heap_empty());}return 0;}
"#,
    );
}

#[test]
fn cell_references_in_native_error_payloads_outlive_the_failing_frame() {
    let mut core = emela_core::Module::new();
    let cell = core.types.intern(Type::Cell(TypeId::INT));
    let error_id = core.add_enum(EnumDef {
        name: "Stored".into(),
        params: 0,
        variants: vec![VariantDef::positional("Stored", vec![cell])],
        error: true,
        external: false,
    });
    let error_ty = core.enum_type(error_id, vec![]);
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "raise");
    b.set_fails();
    let cell_value = b.assign(
        "cell",
        cell,
        Value::Builtin {
            op: Builtin::CellNew,
            args: vec![Const::Int(42).into()],
        },
    );
    let error = b.assign(
        "error",
        error_ty,
        Value::Make {
            ctor: CtorRef {
                enum_id: error_id,
                variant: 0,
            },
            args: vec![cell_value.into()],
        },
    );
    b.term(Term::Fail(error.into()));
    let raise = flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "run");
    b.set_exported().set_ret(TypeId::INT);
    let caught = b.var("caught", TypeId::ANY);
    b.block(Some(caught), "caught", |b, exit| {
        b.let_(
            None,
            Value::Call {
                callee: Callee::Direct(raise),
                args: vec![],
                suspends: false,
                on_fail: Some(OnFail::Break(exit)),
            },
        );
        b.term(Term::Unreachable);
    });
    let concrete = b.assign(
        "concrete",
        error_ty,
        Value::AsError {
            error: caught,
            enum_id: error_id,
        },
    );
    let stored = b.assign(
        "stored",
        cell,
        Value::Field {
            base: concrete,
            ctor: CtorRef {
                enum_id: error_id,
                variant: 0,
            },
            index: 0,
        },
    );
    let result = b.assign(
        "result",
        TypeId::INT,
        Value::Builtin {
            op: Builtin::CellGet,
            args: vec![stored.into()],
        },
    );
    b.term(Term::Return(Some(result.into())));
    flow.add(b.finish());
    let output =
        with_heap_check(compile_flow(&flow, &Options::default().with_heap_bytes(1024)).unwrap());
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m); abort(); }
int main(void){for(unsigned i=0;i<1000;i++){assert(emela_export_run_1()==42);assert(test_heap_empty());}return 0;}
"#,
    );
}

#[test]
fn unit_cells_keep_a_shared_heap_identity_without_reading_a_void_payload() {
    let mut core = emela_core::Module::new();
    let cell = core.types.intern(Type::Cell(TypeId::UNIT));
    let mut b = FnBuilder::new(&core, "run");
    b.set_exported().set_ret(cell);
    let value = b.assign(
        "value",
        cell,
        Value::Builtin {
            op: Builtin::CellNew,
            args: vec![Const::Unit.into()],
        },
    );
    b.let_(
        None,
        Value::Builtin {
            op: Builtin::CellGet,
            args: vec![value.into()],
        },
    );
    b.let_(
        None,
        Value::Builtin {
            op: Builtin::CellSet,
            args: vec![value.into(), Const::Unit.into()],
        },
    );
    b.term(Term::Return(Some(value.into())));
    let function = b.finish();
    let mut flow = Module::new(core);
    flow.add(function);
    let output =
        with_heap_check(compile_flow(&flow, &Options::default().with_heap_bytes(1024)).unwrap());
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m); abort(); }
int main(void){for(unsigned i=0;i<1000;i++){void *value=emela_export_run_0();assert(value);emela_embedded_release(value);assert(test_heap_empty());}return 0;}
"#,
    );
}

#[test]
fn discarded_cell_construction_and_reads_release_their_owned_contents() {
    let mut core = emela_core::Module::new();
    let cell = core.types.intern(Type::Cell(TypeId::INT));
    let mut b = FnBuilder::new(&core, "run");
    b.set_exported();
    let text = b.param("text", TypeId::STRING);
    b.let_(
        None,
        Value::Builtin {
            op: Builtin::CellNew,
            args: vec![text.into()],
        },
    );
    let value = b.assign(
        "value",
        cell,
        Value::Builtin {
            op: Builtin::CellNew,
            args: vec![Const::Int(42).into()],
        },
    );
    b.let_(
        None,
        Value::Builtin {
            op: Builtin::CellGet,
            args: vec![value.into()],
        },
    );
    b.term(Term::Return(None));
    let function = b.finish();
    let mut flow = Module::new(core);
    flow.add(function);
    let output =
        with_heap_check(compile_flow(&flow, &Options::default().with_heap_bytes(1024)).unwrap());
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m); abort(); }
int main(void){for(unsigned i=0;i<1000;i++){emela_export_run_0((EmelaString){"hello",5,NULL});assert(test_heap_empty());}return 0;}
"#,
    );
}
