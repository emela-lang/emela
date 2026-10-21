use super::*;

#[test]
fn native_aggregate_callbacks_own_results_after_captures_and_arguments_are_released() {
    let core = support::try_lower(
        &[(
            "main.emel",
            include_str!("../../examples/support/function_aggregate.emel"),
        )],
        Roots::Exported,
    )
    .unwrap();
    let flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    let (maker, maker_fn) = flow
        .functions
        .iter()
        .find(|(_, f)| f.name == "maker")
        .unwrap();
    let Type::Fn(ft) = &flow.core.types[maker_fn.ret] else {
        panic!()
    };
    let pair = ft.params[0].index();
    let packet = ft.ret.index();
    let callback = maker_fn.ret.index();
    let maker = u32::from(maker.into_raw());
    let native = u32::from(
        flow.functions
            .iter()
            .find(|(_, f)| f.name == "native")
            .unwrap()
            .0
            .into_raw(),
    );
    let mut output = compile(
        &core,
        &Options::default()
            .with_heap_bytes(2048)
            .with_string_capacity(64),
    )
    .unwrap();
    notes::add_heap_check(&mut output);
    let release = aggregate::release_helper(&output, packet);
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
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ fprintf(stderr, "%d: %.*s\n", k,n,m); abort(); }}
int main(void) {{
  for (int i = 0; i < 1000; ++i) {{
    char text[] = {{ 'a', 0, 'b' }}, note[] = "note";
    emela_type_{pair} input = {{ .f0 = {{ .data = text, .length = 3 }}, .f1 = {{ .tag = 0, .payload.v0.f0 = {{.data = note, .length = 4}} }} }};
    emela_function_type_{callback} f = emela_export_maker_{maker}(42);
    emela_type_{packet} result = f.call(f.env, input);
    emela_embedded_release_function_{callback}(&f);
    memset(text, 'X', 3); memset(note, 'X', 4);
    assert(result.tag == 0 && result.payload.v0.f0.f0.owner);
    assert(result.payload.v0.f0.f0.length == 5 && memcmp(result.payload.v0.f0.f0.data, "a\0b42", 5) == 0);
    assert(result.payload.v0.f0.f1.tag == 0 && result.payload.v0.f0.f1.payload.v0.f0.owner);
    assert(memcmp(result.payload.v0.f0.f1.payload.v0.f0.data, "note", 4) == 0);
    {release}(&result);
    result = emela_export_native_{native}(42);
    assert(result.tag == 0 && result.payload.v0.f0.f0.length == 4 && memcmp(result.payload.v0.f0.f0.data, "4342", 4) == 0);
    {release}(&result);
    assert(test_heap_empty());
  }}
}}
"#
        ),
    );
}

#[test]
fn heapless_callbacks_accept_and_return_aggregates_containing_function_values() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
enum Bundle { Call(fn(Int) -> Int, (Int, Bool)) }
fn apply(f: fn(Bundle) -> (Int, Bool), x: Bundle) -> (Int, Bool) { f(x) }
fn unpack(x: Bundle) -> (Int, Bool) {
  x match { Call(f, pair) -> { (n, flag) = pair
    (f(n), flag) } }
}

fn plus(n: Int) -> Int { n + 1 }
pub fn work(n: Int) -> (Int, Bool) { apply(unpack, Call(plus, (n, True))) }
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    let flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    let (id, f) = flow
        .functions
        .iter()
        .find(|(_, f)| f.name == "work")
        .unwrap();
    let ty = f.ret.index();
    let id = u32::from(id.into_raw());
    let output = compile(&core, &Options::default()).unwrap();
    assert!(
        !output
            .files
            .iter()
            .find(|f| f.path == "emela_embedded.c")
            .unwrap()
            .contents
            .contains("emela_heap_storage")
    );
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ (void)k; (void)m; (void)n; abort(); }}
int main(void) {{ emela_type_{ty} value = emela_export_work_{id}(41); assert(value.f0 == 42 && value.f1); }}
"#
        ),
    );
}

#[test]
fn function_signatures_can_refer_back_to_the_aggregate_containing_them() {
    let mut core = emela_core::Module::new();
    let node_id = core.add_enum(EnumDef {
        name: "Node".into(),
        params: 0,
        variants: vec![],
        error: false,
        external: false,
    });
    let node = core.enum_type(node_id, vec![]);
    let callback = core.types.intern(Type::Fn(FnType::pure(vec![node], node)));
    core.enums[node_id].variants = vec![
        VariantDef::positional("Step", vec![callback]),
        VariantDef::unit("End"),
    ];
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "identity");
    b.set_ret(node);
    let value = b.param("value", node);
    b.term(Term::Return(Some(value.into())));
    let identity = flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "work");
    b.set_exported().set_ret(node);
    let f = b.assign("callback", callback, Value::FnRef(identity));
    let value = b.assign(
        "value",
        node,
        Value::Make {
            ctor: CtorRef {
                enum_id: node_id,
                variant: 0,
            },
            args: vec![f.into()],
        },
    );
    let out = b.assign(
        "out",
        node,
        Value::Call {
            callee: Callee::Indirect(f),
            args: vec![value.into()],
            suspends: false,
            on_fail: None,
        },
    );
    b.term(Term::Return(Some(out.into())));
    flow.add(b.finish());
    let output = compile_flow(&flow, &Options::default()).unwrap();
    let ty = node.index();
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ (void)k; (void)m; (void)n; abort(); }}
int main(void) {{
  emela_type_{ty} value = emela_export_work_1(); assert(value.tag == 0);
  emela_type_{ty} end = {{.tag = 1}};
  value = value.payload.v0.f0.call(value.payload.v0.f0.env, end); assert(value.tag == 1);
}}
"#
        ),
    );
}

#[test]
fn aggregate_coercions_preserve_evidence_failure_suspend_and_discard_ownership() {
    let mut core = emela_core::Module::new();
    let pair = core.tuple(vec![TypeId::STRING, TypeId::STRING]);
    let pure = core.types.intern(Type::Fn(FnType::pure(vec![pair], pair)));
    let checked = core.types.intern(Type::Fn(FnType {
        ev: true,
        fails: true,
        ..FnType::pure(vec![pair], pair)
    }));
    let resumable = core.types.intern(Type::Fn(FnType {
        ev: true,
        fails: true,
        suspends: true,
        ..FnType::pure(vec![pair], pair)
    }));
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "identity");
    b.set_ret(pair);
    let value = b.param("value", pair);
    b.term(Term::Return(Some(value.into())));
    let identity = flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "work");
    b.set_exported().set_suspends().set_fails().set_ret(pair);
    let value = b.param("value", pair);
    let f = b.assign("identity", pure, Value::FnRef(identity));
    let checked_f = b.assign("checked", checked, Value::Coerce(f));
    let resumed_f = b.assign("resumable", resumable, Value::Coerce(checked_f));
    let evidence = b.assign(
        "evidence",
        TypeId::EVIDENCE,
        Value::EvExtend {
            base: None,
            bindings: vec![],
        },
    );
    let discarded_sync = b.assign(
        "unused_sync",
        pair,
        Value::Call {
            callee: Callee::Indirect(checked_f),
            args: vec![value.into(), evidence.into()],
            suspends: false,
            on_fail: Some(OnFail::Propagate),
        },
    );
    let discarded_async = b.assign(
        "unused_async",
        pair,
        Value::Call {
            callee: Callee::Indirect(resumed_f),
            args: vec![value.into(), evidence.into()],
            suspends: true,
            on_fail: Some(OnFail::Propagate),
        },
    );
    let result = b.assign(
        "result",
        pair,
        Value::Call {
            callee: Callee::Indirect(resumed_f),
            args: vec![value.into(), evidence.into()],
            suspends: true,
            on_fail: Some(OnFail::Propagate),
        },
    );
    b.term(Term::Return(Some(result.into())));
    let mut f = b.finish();
    for (_, statement) in f.stmts.iter_mut() {
        if let emela_lir::Stmt::Let { dst, .. } = statement
            && (*dst == Some(discarded_sync) || *dst == Some(discarded_async))
        {
            *dst = None;
        }
    }
    flow.add(f);
    let mut output = compile_flow(&flow, &options().with_heap_bytes(1024)).unwrap();
    notes::add_heap_check(&mut output);
    let ty = pair.index();
    let release = aggregate::release_helper(&output, ty);
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
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ fprintf(stderr, "%d: %.*s\n", k,n,m); abort(); }}
int main(void) {{
  for (int i = 0; i < 1000; ++i) {{
    char first[] = {{'a', 0, 'b'}}, second[] = "tail";
    emela_type_{ty} input = {{.f0 = {{.data = first, .length = 3}}, .f1 = {{.data = second, .length = 4}}}}, result = {{0}};
    EmelaFiber f = emela_export_work_1_start(input, i % 3 ? &result : NULL);
    memset(first, 'X', 3); memset(second, 'X', 4);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
    if (i % 3) {{
      assert(result.f0.owner && result.f1.owner);
      assert(result.f0.length == 3 && memcmp(result.f0.data, "a\0b", 3) == 0);
      assert(result.f1.length == 4 && memcmp(result.f1.data, "tail", 4) == 0);
      {release}(&result);
    }}
    assert(test_heap_empty());
  }}
}}
"#
        ),
    );
}

#[test]
fn failing_indirect_aggregate_calls_release_discarded_successes_and_preserve_string_errors() {
    let source = r#"
error Missing(text: String)
fn produce(n: Int, bad: Bool) -> (String, Option[String]) fails Missing {
  if bad { fail Missing(Int.to_string(n)) }
  text = Int.to_string(n + 1)
  (text, Some(text))
}
fn invoke(f: fn(Int, Bool) -> (String, Option[String]) fails Missing, n: Int, bad: Bool) -> (String, Option[String]) fails Missing {
  f(n, False)
  f(n, bad)
}
pub fn checked(n: Int, bad: Bool) -> String {
  pair = invoke(produce, n, bad) escape { Missing(text) -> (text, None) }
  (text, note) = pair
  note match { Some(suffix) -> String.append(text, suffix)
    None -> text }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    let mut output = compile(
        &core,
        &Options::default()
            .with_heap_bytes(2048)
            .with_string_capacity(64),
    )
    .unwrap();
    notes::add_heap_check(&mut output);
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
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k,n,m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; ++i) {
    char buffer[64];
    EmelaString value = emela_export_checked_0(42, i % 2, buffer, 64);
    assert(value.length == (i % 2 ? 2 : 4) && memcmp(value.data, i % 2 ? "42" : "4343", value.length) == 0);
    emela_embedded_release_string(&value);
    assert(test_heap_empty());
  }
}
"#,
    );
}

#[test]
fn signature_cycles_through_nested_tuples_do_not_require_recursive_heap_boxes() {
    let mut core = emela_core::Module::new();
    let id = core.add_enum(EnumDef {
        name: "Node".into(),
        params: 0,
        variants: vec![],
        error: false,
        external: false,
    });
    let node = core.enum_type(id, vec![]);
    let pair = core.tuple(vec![node, TypeId::BOOL]);
    let callback = core.types.intern(Type::Fn(FnType::pure(vec![node], pair)));
    core.enums[id].variants = vec![
        VariantDef::positional("Step", vec![callback]),
        VariantDef::unit("End"),
    ];
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "wrap");
    b.set_ret(pair);
    let value = b.param("value", node);
    let out = b.assign(
        "out",
        pair,
        Value::Tuple(vec![value.into(), Const::Bool(true).into()]),
    );
    b.term(Term::Return(Some(out.into())));
    let wrap = flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "work");
    b.set_exported().set_ret(node);
    let f = b.assign("callback", callback, Value::FnRef(wrap));
    let value = b.assign(
        "value",
        node,
        Value::Make {
            ctor: CtorRef {
                enum_id: id,
                variant: 0,
            },
            args: vec![f.into()],
        },
    );
    b.term(Term::Return(Some(value.into())));
    flow.add(b.finish());
    let output = compile_flow(&flow, &Options::default()).unwrap();
    assert!(
        !output
            .files
            .iter()
            .find(|f| f.path == "emela_embedded.c")
            .unwrap()
            .contents
            .contains("emela_heap_storage")
    );
    let ty = node.index();
    let pair = pair.index();
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ (void)k; (void)m; (void)n; abort(); }}
int main(void) {{
  emela_type_{ty} value = emela_export_work_1(); assert(value.tag == 0);
  emela_type_{pair} pair = value.payload.v0.f0.call(value.payload.v0.f0.env, value);
  assert(pair.f0.tag == 0 && pair.f1);
}}
"#
        ),
    );
}

#[test]
fn growing_generic_recursion_through_a_function_signature_uses_owned_lazy_payloads() {
    let mut core = emela_core::Module::new();
    let id = core.add_enum(EnumDef {
        name: "Grow".into(),
        params: 1,
        variants: vec![],
        error: false,
        external: false,
    });
    let param = core.types.intern(Type::Param(0));
    let list = core.types.intern(Type::List(param));
    let larger = core.enum_type(id, vec![list]);
    let callback = core.types.intern(Type::Fn(FnType::pure(vec![], larger)));
    core.enums[id].variants = vec![
        VariantDef::positional("Step", vec![callback]),
        VariantDef::unit("End"),
    ];
    let node = core.enum_type(id, vec![TypeId::INT]);
    let callback = core.types.subst(callback, &[TypeId::INT]);
    let Type::Fn(ft) = core.types[callback].clone() else {
        unreachable!()
    };
    let larger = ft.ret;
    let mut b = FnBuilder::new(&core, "work");
    b.set_exported().set_ret(node);
    let value = b.param("value", node);
    b.term(Term::Return(Some(value.into())));
    let f = b.finish();
    let mut flow = Module::new(core);
    let identity = flow.add(f);
    let mut b = FnBuilder::new(&flow.core, "stop");
    b.set_ret(larger);
    let end = b.assign(
        "end",
        larger,
        Value::Make {
            ctor: emela_core::CtorRef {
                enum_id: id,
                variant: 1,
            },
            args: vec![],
        },
    );
    b.term(Term::Return(Some(end.into())));
    let stop = flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "run");
    b.set_exported().set_ret(TypeId::INT);
    let f = b.assign("f", callback, Value::FnRef(stop));
    let node_ty = node;
    let node = b.assign(
        "node",
        node_ty,
        Value::Make {
            ctor: emela_core::CtorRef {
                enum_id: id,
                variant: 0,
            },
            args: vec![f.into()],
        },
    );
    let copy = b.assign(
        "copy",
        node_ty,
        Value::Call {
            callee: Callee::Direct(identity),
            args: vec![node.into()],
            suspends: false,
            on_fail: None,
        },
    );
    let f = b.assign(
        "read",
        callback,
        Value::Field {
            base: copy,
            ctor: emela_core::CtorRef {
                enum_id: id,
                variant: 0,
            },
            index: 0,
        },
    );
    let end = b.assign(
        "end",
        larger,
        Value::Call {
            callee: Callee::Indirect(f),
            args: vec![],
            suspends: false,
            on_fail: None,
        },
    );
    let tag = b.assign("tag", TypeId::INT, Value::Tag(end));
    let answer = b.assign(
        "answer",
        TypeId::INT,
        Value::Prim {
            op: PrimOp::Add,
            ty: PrimTy::I32,
            args: vec![tag.into(), Const::Int(41).into()],
        },
    );
    b.term(Term::Return(Some(answer.into())));
    flow.add(b.finish());
    let mut output = compile_flow(&flow, &Options::default().with_heap_bytes(4096)).unwrap();
    notes::add_heap_check(&mut output);
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n){(void)k;(void)m;(void)n;abort();}
int main(void){for(unsigned i=0;i<1000;i++){assert(emela_export_run_2()==42);assert(test_heap_empty());}return 0;}
"#,
    );
}

#[test]
fn aggregate_callbacks_keep_any_fields_complete_and_owned() {
    let mut core = emela_core::Module::new();
    let pair = core.tuple(vec![TypeId::ANY, TypeId::BOOL]);
    let callback = core.types.intern(Type::Fn(FnType::pure(vec![pair], pair)));
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "identity");
    b.set_ret(pair);
    let value = b.param("value", pair);
    b.term(Term::Return(Some(value.into())));
    let identity = flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "work");
    b.set_exported().set_ret(TypeId::INT);
    let number = b.assign("number", TypeId::INT, Value::Const(Const::Int(42)));
    let boxed = b.assign("boxed", TypeId::ANY, Value::Coerce(number));
    let value = b.assign(
        "value",
        pair,
        Value::Tuple(vec![boxed.into(), Const::Bool(true).into()]),
    );
    let f = b.assign("callback", callback, Value::FnRef(identity));
    let result = b.assign(
        "result",
        pair,
        Value::Call {
            callee: Callee::Indirect(f),
            args: vec![value.into()],
            suspends: false,
            on_fail: None,
        },
    );
    let boxed = b.assign(
        "field",
        TypeId::ANY,
        Value::TupleGet {
            base: result,
            index: 0,
        },
    );
    let number = b.assign("number", TypeId::INT, Value::Coerce(boxed));
    b.term(Term::Return(Some(number.into())));
    flow.add(b.finish());
    let mut output = compile_flow(&flow, &Options::default().with_heap_bytes(1024)).unwrap();
    notes::add_heap_check(&mut output);
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
int main(void) { for (int i = 0; i < 1000; ++i) { assert(emela_export_work_1() == 42); assert(test_heap_empty()); } }
"#,
    );
}

#[test]
fn any_fields_nested_in_error_payloads_use_owned_boxes_and_indirect_failure_channels() {
    let mut core = emela_core::Module::new();
    let pair = core.tuple(vec![TypeId::ANY, TypeId::BOOL]);
    let error_id = core.add_enum(EnumDef {
        name: "Missing".into(),
        params: 0,
        variants: vec![VariantDef::positional("Missing", vec![pair])],
        error: true,
        external: false,
    });
    let error_ty = core.enum_type(error_id, vec![]);
    let callback = core.types.intern(Type::Fn(FnType {
        fails: true,
        ..FnType::pure(vec![pair, TypeId::BOOL], pair)
    }));
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "checked");
    b.set_fails().set_ret(pair);
    let value = b.param("value", pair);
    let bad = b.param("bad", TypeId::BOOL);
    b.if_(
        None,
        bad,
        |b| {
            let error = b.assign(
                "error",
                error_ty,
                Value::Make {
                    ctor: CtorRef {
                        enum_id: error_id,
                        variant: 0,
                    },
                    args: vec![value.into()],
                },
            );
            b.term(Term::Fail(error.into()));
        },
        |b| {
            b.term(Term::Return(Some(value.into())));
        },
    );
    b.term(Term::Unreachable);
    let checked = flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "work");
    b.set_exported().set_ret(TypeId::INT);
    let bad = b.param("bad", TypeId::BOOL);
    let number = b.assign("number", TypeId::INT, Value::Const(Const::Int(42)));
    let boxed = b.assign("boxed", TypeId::ANY, Value::Coerce(number));
    let value = b.assign(
        "value",
        pair,
        Value::Tuple(vec![boxed.into(), Const::Bool(true).into()]),
    );
    let f = b.assign("callback", callback, Value::FnRef(checked));
    let caught = b.var("caught", TypeId::ANY);
    b.block(Some(caught), "result", |b, exit| {
        let result = b.assign(
            "result",
            pair,
            Value::Call {
                callee: Callee::Indirect(f),
                args: vec![value.into(), bad.into()],
                suspends: false,
                on_fail: Some(OnFail::Break(exit)),
            },
        );
        let boxed = b.assign("boxed_result", TypeId::ANY, Value::Coerce(result));
        b.term(Term::Break {
            label: exit,
            value: Some(boxed.into()),
        });
    });
    let is_error = b.assign(
        "is_error",
        TypeId::BOOL,
        Value::IsError {
            error: caught,
            enum_id: error_id,
        },
    );
    let out = b.var("out", pair);
    b.if_(
        Some(out),
        is_error,
        |b| {
            let error = b.assign(
                "error",
                error_ty,
                Value::AsError {
                    error: caught,
                    enum_id: error_id,
                },
            );
            let pair = b.assign(
                "pair",
                pair,
                Value::Field {
                    base: error,
                    ctor: CtorRef {
                        enum_id: error_id,
                        variant: 0,
                    },
                    index: 0,
                },
            );
            b.term(Term::Yield(Some(pair.into())));
        },
        |b| {
            let pair = b.assign("pair", pair, Value::Coerce(caught));
            b.term(Term::Yield(Some(pair.into())));
        },
    );
    let boxed = b.assign(
        "field",
        TypeId::ANY,
        Value::TupleGet {
            base: out,
            index: 0,
        },
    );
    let number = b.assign("number", TypeId::INT, Value::Coerce(boxed));
    b.term(Term::Return(Some(number.into())));
    flow.add(b.finish());
    let mut output = compile_flow(&flow, &Options::default().with_heap_bytes(2048)).unwrap();
    notes::add_heap_check(&mut output);
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k,n,m); abort(); }
int main(void) { for (int i = 0; i < 1000; ++i) { assert(emela_export_work_1(i % 2) == 42); assert(test_heap_empty()); } }
"#,
    );
}
