use super::*;
#[path = "../../examples/support/suspend_flow.rs"]
mod suspend_flow;

#[test]
fn suspend_parallel_recur_retains_managed_values_before_replacing_them() {
    let mut flow = suspend_flow::task_flow();
    let wait = flow
        .functions
        .iter()
        .find(|(_, f)| f.name == "wait")
        .unwrap()
        .0;
    let list = flow.core.types.intern(Type::List(TypeId::INT));
    let mut b = FnBuilder::new(&flow.core, "rotate");
    b.set_exported().set_suspends().set_ret(list);
    let n = b.param("n", TypeId::INT);
    let x = b.assign(
        "x",
        list,
        Value::List {
            elems: vec![Const::Int(10).into()],
            tail: None,
        },
    );
    let y = b.assign(
        "y",
        list,
        Value::List {
            elems: vec![Const::Int(20).into()],
            tail: None,
        },
    );
    let a = b.var("a", list);
    let c = b.var("b", list);
    let left = b.var("left", TypeId::INT);
    let result = b.var("result", list);
    b.block(Some(result), "exit", |b, exit| {
        b.loop_(
            None,
            vec![(a, x.into()), (c, y.into()), (left, n.into())],
            |b| {
                b.assign(
                    "tick",
                    TypeId::INT,
                    Value::Call {
                        callee: Callee::Direct(wait),
                        args: vec![left.into()],
                        suspends: true,
                        on_fail: None,
                    },
                );
                let done = b.assign(
                    "done",
                    TypeId::BOOL,
                    Value::Prim {
                        op: PrimOp::Eq,
                        ty: PrimTy::I32,
                        args: vec![left.into(), Const::Int(0).into()],
                    },
                );
                b.if_(
                    None,
                    done,
                    |b| {
                        b.term(Term::Break {
                            label: exit,
                            value: Some(a.into()),
                        })
                    },
                    |b| b.term(Term::Yield(None)),
                );
                let less = b.assign(
                    "less",
                    TypeId::INT,
                    Value::Prim {
                        op: PrimOp::Sub,
                        ty: PrimTy::I32,
                        args: vec![left.into(), Const::Int(1).into()],
                    },
                );
                b.term(Term::Recur(vec![c.into(), a.into(), less.into()]));
            },
        );
        b.term(Term::Unreachable);
    });
    b.term(Term::Return(Some(result.into())));
    flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "head");
    b.set_exported().set_ret(TypeId::INT);
    let value = b.param("value", list);
    let head = b.assign("head", TypeId::INT, Value::ListHead(value));
    b.term(Term::Return(Some(head.into())));
    flow.add(b.finish());
    compile_and_run(
        compile_flow(
            &flow,
            &task_options()
                .with_heap_bytes(2048)
                .with_frame_capacity(4)
                .with_frame_bytes(512),
        )
        .unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
void board_wait_start(EmelaTask t, int32_t n) { assert(emela_task_complete_int(t, n)); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    void *result = NULL;
    EmelaFiber f = emela_export_rotate_2_start(i % 10, &result);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
    assert(emela_export_head_3(result) == (i % 2 ? 20 : 10));
    emela_embedded_release(result);
    f = emela_export_rotate_2_start(5, NULL);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
  }
  return 0;
}
"#,
    );
}

#[test]
fn sync_string_producers_coerced_to_suspend_values_keep_their_backing_and_small_buffer() {
    let mut flow = Module::new(emela_core::Module::new());
    let pure = flow
        .core
        .types
        .intern(Type::Fn(FnType::pure(vec![TypeId::INT], TypeId::STRING)));
    let suspended = flow.core.types.intern(Type::Fn(FnType {
        suspends: true,
        ..FnType::pure(vec![TypeId::INT], TypeId::STRING)
    }));
    let mut b = FnBuilder::new(&flow.core, "format");
    b.set_ret(TypeId::STRING);
    let n = b.param("n", TypeId::INT);
    let text = b.assign(
        "text",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::IntToString,
            args: vec![n.into()],
        },
    );
    b.term(Term::Return(Some(text.into())));
    let format = flow.add(b.finish());
    for discard in [false, true] {
        let mut b = FnBuilder::new(&flow.core, if discard { "discard" } else { "text" });
        b.set_exported().set_suspends().set_ret(if discard {
            TypeId::UNIT
        } else {
            TypeId::STRING
        });
        let n = b.param("n", TypeId::INT);
        let pure = b.assign("pure", pure, Value::FnRef(format));
        let adapted = b.assign("adapted", suspended, Value::Coerce(pure));
        let call = Value::Call {
            callee: Callee::Indirect(adapted),
            args: vec![n.into()],
            suspends: true,
            on_fail: None,
        };
        if discard {
            b.let_(None, call);
            b.term(Term::Return(None));
        } else {
            let result = b.assign("result", TypeId::STRING, call);
            b.term(Term::Return(Some(result.into())));
        }
        flow.add(b.finish());
    }
    let output = compile_flow(&flow, &Options::default().with_heap_bytes(2048)).unwrap();
    let source = &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents;
    assert!(source.contains("char buffer[11];"));
    assert!(!source.contains("char buffer[256];"));
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    EmelaString value = {0};
    EmelaFiber f = emela_export_text_1_start(INT32_MIN, &value);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
    assert(value.owner && value.length == 11 && memcmp(value.data, "-2147483648", 11) == 0);
    emela_embedded_release_string(&value);
    f = emela_export_discard_2_start(INT32_MIN);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
  }
  return 0;
}
"#,
    );
}

#[test]
fn constructing_an_immediate_handler_does_not_emit_a_scheduler() {
    let mut flow = Module::new(emela_core::Module::new());
    let effect = flow.core.add_effect(emela_core::EffectDef::new(
        "Wait",
        vec![emela_core::OpDef {
            name: "get".into(),
            suspend: true,
            type_params: 0,
            params: vec![],
            ret: TypeId::INT,
            fails: false,
        }],
    ));
    let resume_ty = flow
        .core
        .types
        .intern(Type::Fn(FnType::pure(vec![TypeId::INT], TypeId::UNIT)));
    let operation_ty = flow
        .core
        .types
        .intern(Type::Fn(FnType::pure(vec![resume_ty], TypeId::UNIT)));
    let handler_ty = flow.core.types.intern(Type::Handler(effect));
    let mut b = FnBuilder::new(&flow.core, "make");
    b.set_exported().set_ret(handler_ty);
    let op = b.var("operation", operation_ty);
    let resume = b.var("resume", resume_ty);
    b.closure(op, vec![resume], vec![], false, |b| {
        b.let_(
            None,
            Value::Call {
                callee: Callee::Indirect(resume),
                args: vec![Const::Int(42).into()],
                suspends: false,
                on_fail: None,
            },
        );
        b.term(Term::Return(None));
    });
    let handler = b.assign(
        "handler",
        handler_ty,
        Value::Handler {
            name: "Instant".into(),
            effect,
            ops: vec![op],
        },
    );
    b.term(Term::Return(Some(handler.into())));
    flow.add(b.finish());
    let output = compile_flow(&flow, &Options::default().with_heap_bytes(2048)).unwrap();
    assert!(
        output
            .files
            .iter()
            .all(|f| !f.contents.contains("emela_frames[")
                && !f.contents.contains("struct emela_resume {"))
    );
    compile_and_run(
        output,
        r#"
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) emela_embedded_release(emela_export_make_0());
  return 0;
}
"#,
    );
}

#[test]
fn completed_roots_release_waiting_operation_tasks_and_ignore_their_queued_events() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
@external(c, "notify")
fn notify() -> () use {}
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting: Wait {
  suspend fn get(n, resume) {
    resume(n)
    wait(n)
    ()
  }
}
pub fn work(n: Int) -> Int {
  with Waiting {
    value = (use Wait).get(n)
    notify()
    value
  }
}
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    let output = compile(
        &core,
        &task_options()
            .with_heap_bytes(2048)
            .with_frame_capacity(4)
            .with_frame_bytes(512)
            .with_task_capacity(1)
            .with_event_capacity(1),
    )
    .unwrap();
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask pending;
static int queued;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
void board_wait_start(EmelaTask t, int32_t n) { (void)n; pending = t; }
void notify(void) { if (queued) assert(emela_task_complete_int(pending, 0)); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    queued = i % 2;
    int32_t result = -1;
    EmelaFiber f = emela_export_work_0_start(i, &result);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == i && !emela_task_complete_int(pending, 999));
    emela_fiber_release(f);
  }
  while (emela_scheduler_run(1)) {}
  return 0;
}
"#,
    );
}

#[test]
fn coerce_adapts_sync_and_suspend_function_values_to_a_suspend_evidence_abi() {
    let mut flow = suspend_flow::task_flow();
    let work = flow
        .functions
        .iter()
        .find(|(_, f)| f.name == "work")
        .unwrap()
        .0;
    let pure = flow
        .core
        .types
        .intern(Type::Fn(FnType::pure(vec![TypeId::INT], TypeId::INT)));
    let suspended = flow.core.types.intern(Type::Fn(FnType {
        suspends: true,
        ..FnType::pure(vec![TypeId::INT], TypeId::INT)
    }));
    let with_evidence = flow.core.types.intern(Type::Fn(FnType {
        suspends: true,
        ev: true,
        ..FnType::pure(vec![TypeId::INT], TypeId::INT)
    }));
    let mut b = FnBuilder::new(&flow.core, "increment");
    b.set_ret(TypeId::INT);
    let n = b.param("n", TypeId::INT);
    let result = b.assign(
        "result",
        TypeId::INT,
        Value::Prim {
            op: PrimOp::Add,
            ty: PrimTy::I32,
            args: vec![n.into(), Const::Int(1).into()],
        },
    );
    b.term(Term::Return(Some(result.into())));
    let increment = flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "adapted");
    b.set_exported().set_suspends().set_ret(TypeId::INT);
    let n = b.param("n", TypeId::INT);
    let sync = b.assign("sync", pure, Value::FnRef(increment));
    let adapted = b.assign("adapted", with_evidence, Value::Coerce(sync));
    let evidence = b.assign(
        "evidence",
        TypeId::EVIDENCE,
        Value::EvExtend {
            base: None,
            bindings: vec![],
        },
    );
    let first = b.assign(
        "first",
        TypeId::INT,
        Value::Call {
            callee: Callee::Indirect(adapted),
            args: vec![n.into(), evidence.into()],
            suspends: true,
            on_fail: None,
        },
    );
    let asynchronous = b.assign("asynchronous", suspended, Value::FnRef(work));
    let adapted = b.assign("adapted_async", with_evidence, Value::Coerce(asynchronous));
    let result = b.assign(
        "result",
        TypeId::INT,
        Value::Call {
            callee: Callee::Indirect(adapted),
            args: vec![first.into(), evidence.into()],
            suspends: true,
            on_fail: None,
        },
    );
    b.term(Term::Return(Some(result.into())));
    flow.add(b.finish());
    compile_and_run(
        compile_flow(
            &flow,
            &task_options()
                .with_heap_bytes(2048)
                .with_frame_capacity(4)
                .with_frame_bytes(512),
        )
        .unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
void board_wait_start(EmelaTask t, int32_t n) { assert(emela_task_complete_int(t, n)); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t result = -1;
    EmelaFiber f = emela_export_adapted_3_start(i, &result);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == i * 2 + 3); emela_fiber_release(f);
  }
  return 0;
}
"#,
    );
}

fn resume_flow(result: TypeId, return_resume: bool) -> (Module, TypeId) {
    let mut flow = suspend_flow::task_flow();
    let wait = flow
        .functions
        .iter()
        .find(|(_, f)| f.name == "wait")
        .unwrap()
        .0;
    let effect = flow.core.add_effect(emela_core::EffectDef::new(
        "Result",
        vec![emela_core::OpDef {
            name: "get".into(),
            suspend: true,
            type_params: 0,
            params: vec![TypeId::INT],
            ret: result,
            fails: false,
        }],
    ));
    let resume_ty = flow
        .core
        .types
        .intern(Type::Fn(FnType::pure(vec![result], TypeId::UNIT)));
    let op_ty = flow.core.types.intern(Type::Fn(FnType {
        suspends: true,
        ..FnType::pure(vec![TypeId::INT, resume_ty], TypeId::UNIT)
    }));
    let handler_ty = flow.core.types.intern(Type::Handler(effect));
    let mut b = FnBuilder::new(&flow.core, "result");
    b.set_exported()
        .set_suspends()
        .set_ret(if return_resume { resume_ty } else { result });
    let n = b.param("n", TypeId::INT);
    let op = b.var("operation", op_ty);
    let input = b.var("input", TypeId::INT);
    let resume = b.var("resume", resume_ty);
    b.closure(op, vec![input, resume], vec![], true, |b| {
        let waited = b.assign(
            "waited",
            TypeId::INT,
            Value::Call {
                callee: Callee::Direct(wait),
                args: vec![input.into()],
                suspends: true,
                on_fail: None,
            },
        );
        let value = if return_resume {
            b.assign("boxed_resume", TypeId::ANY, Value::Coerce(resume))
        } else {
            b.assign(
                "text",
                TypeId::STRING,
                Value::Builtin {
                    op: Builtin::IntToString,
                    args: vec![waited.into()],
                },
            )
        };
        b.let_(
            None,
            Value::Call {
                callee: Callee::Indirect(resume),
                args: vec![value.into()],
                suspends: false,
                on_fail: None,
            },
        );
        b.term(Term::Return(None));
    });
    let handler = b.assign(
        "handler",
        handler_ty,
        Value::Handler {
            name: "Returning".into(),
            effect,
            ops: vec![op],
        },
    );
    let value = b.assign(
        "value",
        result,
        Value::Call {
            callee: Callee::Op {
                handler,
                effect,
                op: 0,
            },
            args: vec![n.into()],
            suspends: true,
            on_fail: None,
        },
    );
    let value = if return_resume {
        b.assign("callback", resume_ty, Value::Coerce(value))
    } else {
        value
    };
    b.term(Term::Return(Some(value.into())));
    flow.add(b.finish());
    (flow, resume_ty)
}

#[test]
fn resume_retains_string_results_after_the_operation_and_caller_frames_are_recycled() {
    let (flow, _) = resume_flow(TypeId::STRING, false);
    let output = compile_flow(
        &flow,
        &task_options()
            .with_heap_bytes(4096)
            .with_fiber_capacity(4)
            .with_frame_capacity(8)
            .with_frame_bytes(512),
    )
    .unwrap();
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
void board_wait_start(EmelaTask t, int32_t n) { assert(emela_task_complete_int(t, n)); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    EmelaString value = {0};
    EmelaFiber f = emela_export_result_2_start(42, &value);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
    assert(value.owner && value.length == 2 && memcmp(value.data, "42", 2) == 0);
    emela_embedded_release_string(&value);
    f = emela_export_result_2_start(43, NULL);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
  }
  return 0;
}
"#,
    );
}

#[test]
fn late_resume_ignores_completed_and_recycled_root_handles() {
    let (flow, resume_ty) = resume_flow(TypeId::ANY, true);
    let output = compile_flow(
        &flow,
        &task_options()
            .with_heap_bytes(4096)
            .with_fiber_capacity(4)
            .with_frame_capacity(8)
            .with_frame_bytes(512),
    )
    .unwrap();
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
uint32_t emela_board_event_lock(void) {{ return 0; }}
void emela_board_event_unlock(uint32_t p) {{ (void)p; }}
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ (void)k; (void)m; (void)n; abort(); }}
void board_wait_start(EmelaTask t, int32_t n) {{ assert(emela_task_complete_int(t, n)); }}
int main(void) {{
  for (int i = 0; i < 1000; i++) {{
    emela_function_type_{ty} callback = {{0}}, next = {{0}};
    EmelaFiber f = emela_export_result_2_start(1, &callback);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    callback.call(callback.env, (EmelaAny){{0}});
    callback.call(callback.env, (EmelaAny){{0}});
    emela_fiber_release(f);
    EmelaFiber g = emela_export_result_2_start(2, &next);
    callback.call(callback.env, (EmelaAny){{0}});
    while (!emela_fiber_done(g)) assert(emela_scheduler_run(1));
    emela_fiber_release(g);
    emela_embedded_release_function_{ty}(&callback);
    emela_embedded_release_function_{ty}(&next);
  }}
  return 0;
}}
"#,
            ty = resume_ty.index()
        ),
    );
}

#[test]
fn suspend_closure_keeps_captures_after_the_original_function_value_is_released() {
    let mut flow = suspend_flow::task_flow();
    let wait = flow
        .functions
        .iter()
        .find(|(_, f)| f.name == "wait")
        .unwrap()
        .0;
    let closure_ty = flow.core.types.intern(Type::Fn(FnType {
        suspends: true,
        ..FnType::pure(vec![TypeId::INT], TypeId::INT)
    }));
    let list_ty = flow.core.types.intern(Type::List(TypeId::INT));
    let mut b = FnBuilder::new(&flow.core, "make");
    b.set_exported().set_ret(closure_ty);
    let text = b.param("text", TypeId::STRING);
    let list = b.assign(
        "list",
        list_ty,
        Value::List {
            elems: vec![Const::Int(30).into()],
            tail: None,
        },
    );
    let closure = b.var("closure", closure_ty);
    let n = b.var("n", TypeId::INT);
    b.closure(closure, vec![n], vec![text, list], true, |b| {
        let waited = b.assign(
            "waited",
            TypeId::INT,
            Value::Call {
                callee: Callee::Direct(wait),
                args: vec![n.into()],
                suspends: true,
                on_fail: None,
            },
        );
        let length = b.assign(
            "length",
            TypeId::INT,
            Value::Builtin {
                op: Builtin::StringByteSize,
                args: vec![text.into()],
            },
        );
        let head = b.assign("head", TypeId::INT, Value::ListHead(list));
        let sum = b.assign(
            "sum",
            TypeId::INT,
            Value::Prim {
                op: PrimOp::Add,
                ty: PrimTy::I32,
                args: vec![waited.into(), length.into()],
            },
        );
        let result = b.assign(
            "result",
            TypeId::INT,
            Value::Prim {
                op: PrimOp::Add,
                ty: PrimTy::I32,
                args: vec![sum.into(), head.into()],
            },
        );
        b.term(Term::Return(Some(result.into())));
    });
    b.term(Term::Return(Some(closure.into())));
    flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "apply");
    b.set_exported().set_suspends().set_ret(TypeId::INT);
    let closure = b.param("closure", closure_ty);
    let n = b.param("n", TypeId::INT);
    let result = b.assign(
        "result",
        TypeId::INT,
        Value::Call {
            callee: Callee::Indirect(closure),
            args: vec![n.into()],
            suspends: true,
            on_fail: None,
        },
    );
    b.term(Term::Return(Some(result.into())));
    flow.add(b.finish());
    let output = compile_flow(
        &flow,
        &task_options()
            .with_heap_bytes(4096)
            .with_frame_capacity(8)
            .with_frame_bytes(512),
    )
    .unwrap();
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
static int32_t input;
uint32_t emela_board_event_lock(void) {{ return 0; }}
void emela_board_event_unlock(uint32_t p) {{ (void)p; }}
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{
  (void)k; (void)m; (void)n; abort();
}}
void board_wait_start(EmelaTask task, int32_t n) {{ pending = task; input = n; }}
int main(void) {{
  for (int i = 0; i < 1000; i++) {{
    char text[] = "hello";
    emela_function_type_{ty} closure = emela_export_make_2((EmelaString){{text, 5, NULL}});
    memset(text, 'x', 5);
    int32_t result = -1;
    EmelaFiber f = emela_export_apply_3_start(closure, 40, &result);
    emela_embedded_release_function_{ty}(&closure);
    while (emela_scheduler_run(1)) {{ assert(result == -1); }}
    assert(emela_task_complete_int(pending, input));
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == 75);
    emela_fiber_release(f);
  }}
  return 0;
}}
"#,
            ty = closure_ty.index()
        ),
    );
}

#[test]
fn suspend_fnrefs_dispatch_to_external_tasks_and_internal_frames() {
    let mut flow = suspend_flow::task_flow();
    let wait = flow
        .functions
        .iter()
        .find(|(_, f)| f.name == "wait")
        .unwrap()
        .0;
    let work = flow
        .functions
        .iter()
        .find(|(_, f)| f.name == "work")
        .unwrap()
        .0;
    let ty = flow.core.types.intern(Type::Fn(FnType {
        suspends: true,
        ..FnType::pure(vec![TypeId::INT], TypeId::INT)
    }));
    let mut b = FnBuilder::new(&flow.core, "dispatch");
    b.set_exported().set_suspends().set_ret(TypeId::INT);
    let n = b.param("n", TypeId::INT);
    let external = b.assign("external", ty, Value::FnRef(wait));
    let first = b.assign(
        "first",
        TypeId::INT,
        Value::Call {
            callee: Callee::Indirect(external),
            args: vec![n.into()],
            suspends: true,
            on_fail: None,
        },
    );
    let internal = b.assign("internal", ty, Value::FnRef(work));
    let result = b.assign(
        "result",
        TypeId::INT,
        Value::Call {
            callee: Callee::Indirect(internal),
            args: vec![first.into()],
            suspends: true,
            on_fail: None,
        },
    );
    b.term(Term::Return(Some(result.into())));
    flow.add(b.finish());
    compile_and_run(
        compile_flow(&flow, &task_options().with_frame_capacity(4)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
void board_wait_start(EmelaTask t, int32_t n) { assert(emela_task_complete_int(t, n)); }
int main(void) {
  int32_t result = 0;
  EmelaFiber f = emela_export_dispatch_2_start(20, &result);
  while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
  assert(result == 41); emela_fiber_release(f); return 0;
}
"#,
    );
}

#[test]
fn suspend_frames_own_string_inputs_and_return_owned_results_after_reuse() {
    let mut flow = Module::new(emela_core::Module::new());
    let mut b = FnBuilder::new(&flow.core, "echo");
    b.set_suspends().set_ret(TypeId::STRING);
    let text = b.param("text", TypeId::STRING);
    b.term(Term::Return(Some(text.into())));
    let echo = flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "text");
    b.set_exported().set_suspends().set_ret(TypeId::STRING);
    let text = b.param("text", TypeId::STRING);
    let n = b.param("n", TypeId::INT);
    let digits = b.assign(
        "digits",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::IntToString,
            args: vec![n.into()],
        },
    );
    let joined = b.assign(
        "joined",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::StringAppend,
            args: vec![text.into(), digits.into()],
        },
    );
    let result = b.assign(
        "result",
        TypeId::STRING,
        Value::Call {
            callee: Callee::Direct(echo),
            args: vec![joined.into()],
            suspends: true,
            on_fail: None,
        },
    );
    b.term(Term::Return(Some(result.into())));
    flow.add(b.finish());
    let output = compile_flow(
        &flow,
        &Options::default()
            .with_heap_bytes(2048)
            .with_frame_bytes(512),
    )
    .unwrap();
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {
  (void)k; (void)m; (void)n; abort();
}
int main(void) {
  for (int i = 0; i < 1000; i++) {
    char input[] = "value=";
    EmelaString result = {0};
    EmelaFiber f = emela_export_text_1_start((EmelaString){input, 6, NULL}, 42, &result);
    memset(input, 'x', 6);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
    assert(result.owner && result.length == 8 && memcmp(result.data, "value=42", 8) == 0);
    emela_embedded_release_string(&result);
    f = emela_export_text_1_start((EmelaString){"discard=", 8, NULL}, 1, NULL);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(2));
    emela_fiber_release(f);
  }
  return 0;
}
"#,
    );
}

#[test]
fn suspend_recursion_resumes_into_caller_storage_and_reuses_frames() {
    let options = Options::default()
        .with_fiber_capacity(2)
        .with_frame_capacity(64)
        .with_frame_bytes(128);
    let output = compile_flow(&suspend_flow::recursive_flow(), &options)
        .expect("emit recursive state machine");
    let source = &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents;
    assert!(!source.contains("emela_heap_storage"));
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
  (void)kind; (void)message; (void)length; abort();
}
int main(void) {
  for (unsigned repeat = 0; repeat < 1000; repeat++) {
    int32_t a = -1, b = -1;
    EmelaFiber x = emela_export_descend_0_start(30, &a);
    EmelaFiber y = emela_export_descend_0_start(4, &b);
    assert(emela_scheduler_run(0) == 0 && a == -1 && b == -1);
    assert(emela_scheduler_run(1) == 1 && a == -1 && b == -1);
    unsigned steps = 1;
    while (!emela_fiber_done(x) || !emela_fiber_done(y)) {
      steps += emela_scheduler_run(3);
      assert(steps <= 100);
    }
    assert(a == 465 && b == 10);
    emela_fiber_release(x); emela_fiber_release(y);
  }
  return 0;
}
"#,
    );
}

#[test]
fn suspend_loop_preserves_parallel_recur_and_block_result() {
    let mut flow = Module::new(emela_core::Module::new());
    let identity = {
        let mut b = FnBuilder::new(&flow.core, "identity");
        b.set_suspends().set_ret(TypeId::INT);
        let x = b.param("x", TypeId::INT);
        b.term(Term::Return(Some(x.into())));
        b.finish()
    };
    let identity = flow.add(identity);
    let mut b = FnBuilder::new(&flow.core, "rotate");
    b.set_exported().set_suspends().set_ret(TypeId::INT);
    let n = b.param("n", TypeId::INT);
    let a = b.var("a", TypeId::INT);
    let c = b.var("c", TypeId::INT);
    let left = b.var("left", TypeId::INT);
    let result = b.var("result", TypeId::INT);
    b.block(Some(result), "exit", |b, exit| {
        b.loop_(
            None,
            vec![
                (a, Const::Int(10).into()),
                (c, Const::Int(20).into()),
                (left, n.into()),
            ],
            |b| {
                let x = b.assign(
                    "x",
                    TypeId::INT,
                    Value::Call {
                        callee: Callee::Direct(identity),
                        args: vec![a.into()],
                        suspends: true,
                        on_fail: None,
                    },
                );
                let done = b.assign(
                    "done",
                    TypeId::BOOL,
                    Value::Prim {
                        op: PrimOp::Eq,
                        ty: PrimTy::I32,
                        args: vec![left.into(), Const::Int(0).into()],
                    },
                );
                b.if_(
                    None,
                    done,
                    |b| {
                        b.term(Term::Break {
                            label: exit,
                            value: Some(x.into()),
                        })
                    },
                    |b| b.term(Term::Yield(None)),
                );
                let less = b.assign(
                    "less",
                    TypeId::INT,
                    Value::Prim {
                        op: PrimOp::Sub,
                        ty: PrimTy::I32,
                        args: vec![left.into(), Const::Int(1).into()],
                    },
                );
                b.term(Term::Recur(vec![c.into(), a.into(), less.into()]));
            },
        );
        b.term(Term::Unreachable);
    });
    b.term(Term::Return(Some(result.into())));
    let function = b.finish();
    flow.add(function);
    let output = compile_flow(&flow, &Options::default()).expect("emit suspend loop");
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
  (void)kind; (void)message; (void)length; abort();
}
int main(void) {
  for (int32_t n = 0; n < 100; n++) {
    int32_t result = 0;
    EmelaFiber f = emela_export_rotate_1_start(n, &result);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1) == 1);
    assert(result == (n % 2 ? 20 : 10));
    emela_fiber_release(f);
  }
  return 0;
}
"#,
    );
}

#[test]
fn synchronous_program_omits_scheduler_storage_and_zero_suspend_pool_is_diagnosed() {
    let output = compile_constant_function("answer", TypeId::INT, Const::Int(42));
    assert!(
        output
            .files
            .iter()
            .all(|f| !f.contents.contains("emela_frames") && !f.contents.contains("EmelaFiber"))
    );
    let errors = compile_flow(
        &suspend_flow::recursive_flow(),
        &Options::default().with_frame_capacity(0),
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.kind == ErrorKind::InvalidOptions)
    );
}

#[test]
fn suspend_frame_exhaustion_is_a_defect_and_isr_entry_is_rejected() {
    let flow = suspend_flow::recursive_flow();
    let errors = compile_flow(&flow, &Options::default().with_isr_entry("descend")).unwrap_err();
    assert!(errors.iter().any(|error| error.message.contains("ISR")));
    let output = compile_flow(&flow, &Options::default().with_frame_capacity(2)).unwrap();
    compile_and_run(
        output,
        r#"
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
  exit(kind == EMELA_DEFECT_HEAP_EXHAUSTED && length == 30 &&
       memcmp(message, "fixed scheduler pool exhausted", length) == 0 ? 0 : 2);
}
int main(void) {
  int32_t result = 0;
  (void)emela_export_descend_0_start(3, &result);
  (void)emela_scheduler_run(10);
  return 1;
}
"#,
    );
}

fn task_options() -> Options {
    Options::default()
        .with_task_adapter("host_wait", "board_wait_start")
        .with_fiber_capacity(2)
        .with_frame_capacity(2)
        .with_frame_bytes(128)
        .with_task_capacity(2)
        .with_event_capacity(2)
}

#[test]
fn immediate_resume_finishes_the_operation_body_before_the_caller_continues() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
@external(c, "mark")
fn mark(n: Int) -> () use {}
effect Wait { suspend fn get(n: Int) -> Int }
handler Instant: Wait {
  suspend fn get(n, resume) {
    mark(1)
    resume(n)
    mark(2)
  }
}

pub fn instant(n: Int) -> Int {
  with Instant {
    value = (use Wait).get(n)
    mark(3)
    value + 1
  }
}
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    let output = compile(&core, &Options::default().with_heap_bytes(2048)).unwrap();
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
static unsigned order;
void mark(int32_t n) { assert(n == (int32_t)++order); }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) { order = 0; assert(emela_export_instant_0(i) == i + 1); assert(order == 3); }
  return 0;
}
"#,
    );
}

#[test]
fn delayed_resume_runs_the_caller_until_its_next_suspension() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
@external(c, "mark")
fn mark(n: Int) -> () use {}
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting: Wait {
  suspend fn get(n, resume) {
    mark(n * 10 + 1)
    value = wait(n)
    mark(n * 10 + 2)
    callback = fn() { resume(value) }
    callback()
    mark(n * 10 + 4)
  }
}
pub fn work() -> Int {
  with Waiting {
    a = (use Wait).get(1)
    mark(13)
    b = (use Wait).get(2)
    mark(23)
    a + b
  }
}
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    let output = compile(
        &core,
        &task_options()
            .with_heap_bytes(4096)
            .with_fiber_capacity(6)
            .with_frame_capacity(12)
            .with_frame_bytes(512),
    )
    .unwrap();
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask pending;
static unsigned count;
static const int order[] = {11, 12, 13, 21, 14, 22, 23};
void mark(int32_t n) { assert(count < sizeof(order) / sizeof(order[0]) && n == order[count++]); }
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
void board_wait_start(EmelaTask t, int32_t n) { (void)n; pending = t; }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    count = 0;
    int32_t result = -1;
    EmelaFiber f = emela_export_work_0_start(&result);
    while (emela_scheduler_run(1)) assert(result == -1);
    assert(count == 1);
    assert(emela_task_complete_int(pending, 1));
    while (emela_scheduler_run(1)) assert(result == -1);
    assert(count == 5);
    assert(emela_task_complete_int(pending, 2));
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == 3 && count == 7);
    emela_fiber_release(f);
  }
  return 0;
}
"#,
    );
}

#[test]
fn duplicate_resume_reports_the_operation_and_handler() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
effect Wait { suspend fn get(n: Int) -> Int }
handler Twice: Wait {
  suspend fn get(n, resume) {
    resume(n)
    resume(n)
  }
}
pub fn work() -> Int { with Twice { (use Wait).get(42) } }
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    compile_and_run(
        compile(&core, &Options::default().with_heap_bytes(2048)).unwrap(),
        r#"
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {
  static const char expected[] = "resume called twice in operation `get` of handler `Twice`";
  exit(k == EMELA_DEFECT_PANIC && n == sizeof(expected) - 1 && memcmp(m, expected, n) == 0 ? 0 : 2);
}
int main(void) { (void)emela_export_work_0(); return 1; }
"#,
    );
}

#[test]
fn suspend_tasks_queue_completion_without_resuming_in_the_callback() {
    let output =
        compile_flow(&suspend_flow::task_flow(), &task_options()).expect("emit task calls");
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
static unsigned locked;
static EmelaTask requests[4];
static int32_t inputs[4];
static unsigned count;
static int immediate;
uint32_t emela_board_event_lock(void) { unsigned old = locked; locked = 1; return old; }
void emela_board_event_unlock(uint32_t old) { assert(locked); locked = old; }
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
  (void)kind; (void)message; (void)length; abort();
}
void board_wait_start(EmelaTask task, int32_t n) {
  assert(!locked);
  if (immediate) { assert(emela_task_complete_int(task, n)); return; }
  assert(count < 4); requests[count] = task; inputs[count++] = n;
}
int main(void) {
  int32_t a = -1, b = -1;
  EmelaFiber x = emela_export_work_1_start(40, &a);
  EmelaFiber y = emela_export_work_1_start(10, &b);
  assert(emela_scheduler_run(2) == 2 && count == 2);
  assert(emela_scheduler_run(10) == 0 && a == -1 && b == -1);
  assert(!emela_task_complete_float(requests[1], 10.0));
  assert(emela_task_complete_int(requests[1], inputs[1]));
  assert(!emela_task_complete_int(requests[1], 999));
  assert(!locked && count == 2 && b == -1);
  assert(emela_scheduler_run(1) == 1 && count == 2);
  assert(emela_scheduler_run(1) == 1 && count == 3);
  assert(!emela_task_complete_int(requests[1], 999));
  assert(emela_task_complete_int(requests[2], inputs[2]));
  assert(emela_scheduler_run(2) == 2 && b == 21);
  assert(emela_fiber_done(y) && !emela_fiber_done(x));
  emela_fiber_release(y);
  assert(emela_task_complete_int(requests[0], inputs[0]));
  assert(emela_scheduler_run(2) == 2 && count == 4);
  assert(emela_task_complete_int(requests[3], inputs[3]));
  assert(emela_scheduler_run(2) == 2 && a == 81);
  emela_fiber_release(x);
  assert(!emela_task_complete_int((EmelaTask){UINT32_MAX, 0}, 1));
  immediate = 1;
  for (int32_t n = 0; n < 1000; n++) {
    int32_t result = -1;
    EmelaFiber f = emela_export_work_1_start(n, &result);
    unsigned steps = 0;
    while (!emela_fiber_done(f)) {
      assert(emela_scheduler_run(1) == 1);
      assert(++steps <= 5);
    }
    assert(result == n * 2 + 1 && !locked);
    emela_fiber_release(f);
  }
  return 0;
}
"#,
    );
}

#[test]
fn suspend_task_abi_requires_an_explicit_adapter_and_nonzero_pools() {
    let flow = suspend_flow::task_flow();
    let errors = compile_flow(&flow, &Options::default()).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("with_task_adapter"))
    );
    for options in [
        task_options().with_task_capacity(0),
        task_options().with_event_capacity(0),
    ] {
        assert!(
            compile_flow(&flow, &options)
                .unwrap_err()
                .iter()
                .any(|e| e.kind == ErrorKind::InvalidOptions)
        );
    }
}

#[test]
fn suspend_task_and_event_exhaustion_restore_the_board_lock_before_defect() {
    let flow = suspend_flow::task_flow();
    let harness = r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
static int locked;
static EmelaTask pending[2];
static unsigned count;
uint32_t emela_board_event_lock(void) { int old = locked; locked = 1; return old; }
void emela_board_event_unlock(uint32_t old) { assert(locked); locked = (int)old; }
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
  (void)message; (void)length;
  exit(kind == EMELA_DEFECT_HEAP_EXHAUSTED && !locked ? 0 : 2);
}
void board_wait_start(EmelaTask task, int32_t input) { (void)input; pending[count++] = task; }
int main(void) {
  static int32_t a, b;
  (void)emela_export_work_1_start(1, &a);
  (void)emela_export_work_1_start(2, &b);
  (void)emela_scheduler_run(2);
  assert(count == 2);
  assert(emela_task_complete_int(pending[0], 1));
  (void)emela_task_complete_int(pending[1], 2);
  return 1;
}
"#;
    for options in [
        task_options().with_task_capacity(1),
        task_options().with_event_capacity(1),
    ] {
        compile_and_run(compile_flow(&flow, &options).unwrap(), harness);
    }
}

#[test]
fn suspend_handler_source_waits_and_resumes_into_the_callers_result() {
    let core = support::try_lower(
        &[(
            "main.emel",
            include_str!("../../examples/support/suspend_handler.emel"),
        )],
        Roots::Exported,
    )
    .expect("lower a valid suspend handler program");
    let output = compile(
        &core,
        &task_options()
            .with_heap_bytes(2048)
            .with_fiber_capacity(8)
            .with_frame_capacity(12)
            .with_frame_bytes(512),
    )
    .unwrap();
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask pending[2];
static int32_t inputs[2];
static unsigned count;
static int immediate;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
void board_wait_start(EmelaTask task, int32_t n) {
  if (immediate) { assert(emela_task_complete_int(task, n)); return; }
  assert(count < 2); pending[count] = task; inputs[count++] = n;
}
int main(void) {
  int32_t a = -1, b = -1;
  EmelaFiber x = emela_export_work_0_start(40, &a);
  EmelaFiber y = emela_export_work_0_start(10, &b);
  while (emela_scheduler_run(1)) assert(a == -1 && b == -1);
  assert(count == 2);
  assert(emela_task_complete_int(pending[1], inputs[1]));
  while (!emela_fiber_done(y)) assert(emela_scheduler_run(1));
  assert(a == -1 && b == 11); emela_fiber_release(y);
  assert(emela_task_complete_int(pending[0], inputs[0]));
  while (!emela_fiber_done(x)) assert(emela_scheduler_run(1));
  assert(a == 41); emela_fiber_release(x);
  immediate = 1;
  for (int i = 0; i < 1000; i++) {
    int32_t result = -1;
    EmelaFiber f = emela_export_work_0_start(i, &result);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == i + 1); emela_fiber_release(f);
  }
  return 0;
}
"#,
    );
}

#[test]
fn suspend_task_completion_preserves_int64_float_bool_and_unit_abis() {
    for (ty, c_type, input, complete, check) in [
        (
            TypeId::INT64,
            "int64_t",
            "INT64_MIN",
            "emela_task_complete_int64",
            "result == INT64_MIN",
        ),
        (
            TypeId::FLOAT,
            "double",
            "-0.0",
            "emela_task_complete_float",
            "result == 0.0 && signbit(result)",
        ),
        (
            TypeId::BOOL,
            "int32_t",
            "1",
            "emela_task_complete_int",
            "result == 1",
        ),
        (TypeId::UNIT, "", "", "emela_task_complete_unit", "1"),
    ] {
        let mut flow = Module::new(emela_core::Module::new());
        let mut b = FnBuilder::new(&flow.core, "echo");
        b.set_suspends().set_ret(ty);
        if ty != TypeId::UNIT {
            b.param("input", ty);
        }
        let external = b.finish_external(External {
            bindings: vec![ExternalBinding {
                target: "c".into(),
                host: HostRef::Global("host_echo".into()),
            }],
        });
        let echo = flow.add(external);
        let mut b = FnBuilder::new(&flow.core, "work");
        b.set_exported().set_suspends().set_ret(ty);
        let args = if ty == TypeId::UNIT {
            vec![]
        } else {
            vec![b.param("input", ty).into()]
        };
        let value = Value::Call {
            callee: Callee::Direct(echo),
            args,
            suspends: true,
            on_fail: None,
        };
        if ty == TypeId::UNIT {
            b.let_(None, value);
            b.term(Term::Return(None));
        } else {
            let result = b.assign("result", ty, value);
            b.term(Term::Return(Some(result.into())));
        }
        flow.add(b.finish());
        let output = compile_flow(
            &flow,
            &Options::default().with_task_adapter("host_echo", "board_echo"),
        )
        .unwrap();
        let (extra_param, completion_args, result_decl, start_args) = if ty == TypeId::UNIT {
            (String::new(), String::new(), String::new(), String::new())
        } else {
            (
                format!(", {c_type} input"),
                ", input".into(),
                format!("{c_type} result = 0;"),
                format!("{input}, &result"),
            )
        };
        compile_and_run(
            output,
            &format!(
                r#"
#include <assert.h>
#include <math.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
uint32_t emela_board_event_lock(void) {{ return 0; }}
void emela_board_event_unlock(uint32_t previous) {{ (void)previous; }}
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {{
  (void)kind; (void)message; (void)length; abort();
}}
void board_echo(EmelaTask task{extra_param}) {{ assert({complete}(task{completion_args})); }}
int main(void) {{
  {result_decl}
  EmelaFiber fiber = emela_export_work_1_start({start_args});
  assert(emela_scheduler_run(3) == 3 && emela_fiber_done(fiber));
  assert({check});
  emela_fiber_release(fiber);
  return 0;
}}
"#
            ),
        );
    }
}

#[test]
fn suspend_switch_handles_payload_and_nullary_tags() {
    let mut core = emela_core::Module::new();
    let payload = core.add_enum(EnumDef {
        name: "Choice".into(),
        params: 0,
        variants: vec![
            VariantDef::positional("Some", vec![TypeId::INT]),
            VariantDef::unit("None"),
        ],
        error: false,
        external: false,
    });
    let color = core.add_enum(EnumDef {
        name: "Color".into(),
        params: 0,
        variants: vec![VariantDef::unit("Red"), VariantDef::unit("Blue")],
        error: false,
        external: false,
    });
    let payload = core.enum_type(payload, vec![]);
    let color = core.enum_type(color, vec![]);
    let mut flow = Module::new(core);
    for (name, ty) in [("choice", payload), ("color", color)] {
        let mut b = FnBuilder::new(&flow.core, name);
        b.set_exported().set_suspends().set_ret(TypeId::INT);
        let input = b.param("input", ty);
        let first = b.seq(|b| b.term(Term::Yield(Some(Const::Int(11).into()))));
        let second = b.seq(|b| b.term(Term::Yield(Some(Const::Int(22).into()))));
        let result = b.var("result", TypeId::INT);
        b.switch(
            Some(result),
            SwitchOn::Tag(input),
            vec![(CaseKey::Variant(0), first), (CaseKey::Variant(1), second)],
            None,
        );
        b.term(Term::Return(Some(result.into())));
        flow.add(b.finish());
    }
    compile_and_run(
        compile_flow(&flow, &Options::default()).unwrap(),
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
static int expected;
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ (void)m; assert(expected && k == EMELA_DEFECT_NO_MATCH && n == 8); exit(0); }}
int main(void) {{
  for (int i = 0; i < 1000; i++) {{
    int32_t a = 0, b = 0;
    emela_type_{ty} value = {{.tag = i % 2}};
    EmelaFiber f = emela_export_choice_0_start(value, &a), g = emela_export_color_1_start(i % 2, &b);
    while (!emela_fiber_done(f) || !emela_fiber_done(g)) assert(emela_scheduler_run(1));
    assert(a == (i % 2 ? 22 : 11) && a == b);
    emela_fiber_release(f); emela_fiber_release(g);
  }}
  expected = 1;
  emela_type_{ty} invalid = {{.tag = 99}};
  EmelaFiber f = emela_export_choice_0_start(invalid, NULL);
  while (!emela_fiber_done(f)) emela_scheduler_run(1);
  abort();
}}
"#,
            ty = payload.index()
        ),
    );
}
