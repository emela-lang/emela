use super::*;

fn fixture() -> (Output, usize, usize) {
    let core = support::try_lower(
        &[(
            "main.emel",
            include_str!("../../examples/support/aggregate_suspend.emel"),
        )],
        Roots::Public,
    )
    .unwrap();
    let flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    let result = flow
        .functions
        .iter()
        .find(|(_, f)| f.name == "make")
        .unwrap()
        .1
        .ret
        .index();
    let callback = flow
        .functions
        .iter()
        .find(|(_, f)| f.name == "maker")
        .unwrap()
        .1
        .ret
        .index();
    let mut output = compile(
        &core,
        &options().with_string_capacity(64).with_frame_bytes(768),
    )
    .unwrap();
    notes::add_heap_check(&mut output);
    output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents
        .push_str("void test_cancel(EmelaFiber h) { emela_fiber_get(h)->cancelled = 1; }\n");
    (output, result, callback)
}

#[test]
fn deferred_complete_poll_and_await_share_owned_string_aggregates_past_scope_end() {
    let core = support::try_lower(
        &[(
            "main.emel",
            include_str!("../../examples/support/deferred_aggregate.emel"),
        )],
        Roots::Public,
    )
    .unwrap();
    let flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    let ty = flow
        .functions
        .iter()
        .find(|(_, f)| f.name == "head")
        .unwrap()
        .1
        .ret
        .index();
    let mut output = compile(
        &core,
        &Options::default()
            .with_heap_bytes(4096)
            .with_fiber_capacity(8)
            .with_frame_capacity(24)
            .with_frame_bytes(768)
            .with_string_capacity(64)
            .with_deferred_capacity(1),
    )
    .unwrap();
    notes::add_heap_check(&mut output);
    let release = release_helper(&output, ty);
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
    int cancel = i % 2, discard = i % 3 == 0;
    void *list = NULL;
    EmelaFiber f = emela_export_work_0_start(42, cancel, discard ? NULL : &list);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
    if (!discard && !cancel) {{
      emela_type_{ty} polled = emela_export_head_1(list);
      void *rest = emela_export_tail_2(list);
      emela_type_{ty} awaited = emela_export_head_1(rest);
      void *last = emela_export_tail_2(rest);
      emela_type_{ty} captured = emela_export_head_1(last);
      emela_embedded_release(last); emela_embedded_release(rest); emela_embedded_release(list);
      assert(polled.f0.owner && polled.f0.owner == awaited.f0.owner);
      assert(polled.f1.tag == 0 && polled.f1.payload.v0.f0.owner == awaited.f1.payload.v0.f0.owner);
      assert(polled.f0.length == 2 && memcmp(polled.f0.data, "42", 2) == 0);
      assert(polled.f1.payload.v0.f0.length == 2 && memcmp(polled.f1.payload.v0.f0.data, "42", 2) == 0);
      {release}(&polled);
      assert(awaited.f0.length == 2 && memcmp(awaited.f0.data, "42", 2) == 0);
      assert(captured.f0.length == 2 && memcmp(captured.f0.data, "52", 2) == 0);
      assert(captured.f1.tag == 0 && captured.f1.payload.v0.f0.length == 2 && memcmp(captured.f1.payload.v0.f0.data, "53", 2) == 0);
      {release}(&awaited); {release}(&captured);
    }} else assert(list == NULL);
    assert(test_heap_empty());
  }}
}}
"#
        ),
    );
}

#[test]
fn parallel_recur_and_discarded_native_calls_reclaim_string_tuple_results() {
    let mut core = emela_core::Module::new();
    let pair = core.tuple(vec![TypeId::STRING, TypeId::STRING]);
    let missing = core.add_enum(EnumDef {
        name: "Missing".into(),
        params: 0,
        variants: vec![VariantDef::unit("Missing")],
        error: true,
        external: false,
    });
    let error_ty = core.enum_type(missing, vec![]);
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "make");
    b.set_ret(pair);
    let n = b.param("n", TypeId::INT);
    let text = b.assign(
        "text",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::IntToString,
            args: vec![n.into()],
        },
    );
    let value = b.assign("pair", pair, Value::Tuple(vec![text.into(), text.into()]));
    b.term(Term::Return(Some(value.into())));
    let make = flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "fallible");
    b.set_ret(pair).set_fails();
    let n = b.param("n", TypeId::INT);
    let negative = b.assign(
        "negative",
        TypeId::BOOL,
        Value::Prim {
            op: PrimOp::Lt,
            ty: PrimTy::I32,
            args: vec![n.into(), Const::Int(0).into()],
        },
    );
    b.if_(
        None,
        negative,
        |b| {
            let error = b.assign(
                "error",
                error_ty,
                Value::Make {
                    ctor: CtorRef {
                        enum_id: missing,
                        variant: 0,
                    },
                    args: vec![],
                },
            );
            b.term(Term::Fail(error.into()));
        },
        |b| b.term(Term::Yield(None)),
    );
    let value = b.assign(
        "value",
        pair,
        Value::Call {
            callee: Callee::Direct(make),
            args: vec![n.into()],
            suspends: false,
            on_fail: None,
        },
    );
    b.term(Term::Return(Some(value.into())));
    let fallible = flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "wait");
    b.set_ret(TypeId::INT).set_suspends();
    b.param("n", TypeId::INT);
    let wait = flow.add(b.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("host_wait".into()),
        }],
    }));
    let mut b = FnBuilder::new(&flow.core, "rotate");
    b.set_exported().set_suspends().set_ret(pair);
    let n = b.param("n", TypeId::INT);
    for input in [30, -1] {
        let caught = b.var("caught", TypeId::ANY);
        b.block(None, "done", |b, done| {
            b.block(Some(caught), "caught", |b, caught| {
                b.let_(
                    None,
                    Value::Call {
                        callee: Callee::Direct(fallible),
                        args: vec![Const::Int(input).into()],
                        suspends: false,
                        on_fail: Some(OnFail::Break(caught)),
                    },
                );
                b.term(Term::Break {
                    label: done,
                    value: None,
                });
            });
            b.term(Term::Yield(None));
        });
    }
    let x = b.assign(
        "x",
        pair,
        Value::Call {
            callee: Callee::Direct(make),
            args: vec![Const::Int(10).into()],
            suspends: false,
            on_fail: None,
        },
    );
    let y = b.assign(
        "y",
        pair,
        Value::Call {
            callee: Callee::Direct(make),
            args: vec![Const::Int(20).into()],
            suspends: false,
            on_fail: None,
        },
    );
    let a = b.var("a", pair);
    let c = b.var("c", pair);
    let left = b.var("left", TypeId::INT);
    let result = b.var("result", pair);
    b.block(Some(result), "exit", |b, exit| {
        b.loop_(
            None,
            vec![(a, x.into()), (c, y.into()), (left, n.into())],
            |b| {
                b.let_(
                    None,
                    Value::Call {
                        callee: Callee::Direct(make),
                        args: vec![Const::Int(99).into()],
                        suspends: false,
                        on_fail: None,
                    },
                );
                b.assign(
                    "waited",
                    TypeId::INT,
                    Value::Call {
                        callee: Callee::Direct(wait),
                        args: vec![left.into()],
                        suspends: true,
                        on_fail: None,
                    },
                );
                let zero = b.assign(
                    "zero",
                    TypeId::BOOL,
                    Value::Prim {
                        op: PrimOp::Eq,
                        ty: PrimTy::I32,
                        args: vec![left.into(), Const::Int(0).into()],
                    },
                );
                b.if_(
                    None,
                    zero,
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
    let mut output = compile_flow(
        &flow,
        &options()
            .with_heap_bytes(1024)
            .with_fiber_capacity(1)
            .with_frame_capacity(3)
            .with_frame_bytes(768)
            .with_task_capacity(1)
            .with_event_capacity(1),
    )
    .unwrap();
    notes::add_heap_check(&mut output);
    let release = release_helper(&output, pair.index());
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
uint32_t emela_board_event_lock(void) {{ return 0; }}
void emela_board_event_unlock(uint32_t p) {{ (void)p; }}
void board_wait_start(EmelaTask t, int32_t n) {{ assert(emela_task_complete_int(t, n)); }}
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ fprintf(stderr, "%d: %.*s\n", k,n,m); abort(); }}
int main(void) {{
  for (int i = 0; i < 1000; ++i) {{
    emela_type_{ty} result = {{0}};
    int n = i == 0 ? 1000 : i % 8;
    EmelaFiber f = emela_export_rotate_3_start(n, i % 3 ? &result : NULL);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(8));
    emela_fiber_release(f);
    if (i % 3) {{
      assert(result.f0.owner && result.f1.owner && result.f0.length == 2 && result.f1.length == 2);
      assert(memcmp(result.f0.data, n % 2 ? "20" : "10", 2) == 0 && memcmp(result.f0.data, result.f1.data, 2) == 0);
      {release}(&result);
    }}
    assert(test_heap_empty());
  }}
}}
"#,
            ty = pair.index()
        ),
    );
}

#[test]
fn native_string_enum_producers_use_owned_backing_and_heapless_borrowed_tuples_stay_borrowed() {
    let core = support::try_lower(
        &[(
            "main.emel",
            "pub fn encoded(n: Int) -> Option[String] { String.from_code_point(n) }\n",
        )],
        Roots::Public,
    )
    .unwrap();
    let flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    let ty = flow
        .functions
        .iter()
        .find(|(_, f)| f.name == "encoded")
        .unwrap()
        .1
        .ret
        .index();
    let errors = compile(&core, &Options::default()).expect_err("owned producer requires a heap");
    assert!(
        errors
            .iter()
            .any(|e| e.kind == ErrorKind::Unsupported && e.message.contains("heap_bytes = 0"))
    );
    let mut output = compile(&core, &Options::default().with_heap_bytes(256)).unwrap();
    let release = release_helper(&output, ty);
    notes::add_heap_check(&mut output);
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
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ (void)k; (void)m; (void)n; abort(); }}
int main(void) {{
  for (int i = 0; i < 1000; i++) {{
    emela_type_{ty} value = emela_export_encoded_0(i % 2 ? 0x1f600 : 0);
    assert(value.tag == 0 && value.payload.v0.f0.owner);
    assert(value.payload.v0.f0.length == (i % 2 ? 4 : 1));
    assert(memcmp(value.payload.v0.f0.data, i % 2 ? "\xf0\x9f\x98\x80" : "\0", value.payload.v0.f0.length) == 0);
    {release}(&value);
    value = emela_export_encoded_0(0xd800);
    assert(value.tag == 1); {release}(&value);
    assert(test_heap_empty());
  }}
}}
"#
        ),
    );

    let core = support::try_lower(&[("main.emel", "pub fn identity(pair: (String, Option[String])) -> (String, Option[String]) { pair }\n")], Roots::Public).unwrap();
    let flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    let ty = flow
        .functions
        .iter()
        .find(|(_, f)| f.name == "identity")
        .unwrap()
        .1
        .ret
        .index();
    let output = compile(&core, &Options::default()).unwrap();
    assert!(
        !output
            .files
            .iter()
            .any(|f| f.contents.contains("emela_heap_storage"))
    );
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include "emela_embedded.h"
int main(void) {{
  char data[] = "abc";
  emela_type_{ty} input = {{.f0 = {{.data = data, .length = 3}}, .f1 = {{.tag = 1}}}};
  emela_type_{ty} result = emela_export_identity_0(input);
  assert(result.f0.data == data && !result.f0.owner && result.f1.tag == 1);
}}
"#
        ),
    );
}

pub(super) fn release_helper(output: &Output, ty: usize) -> String {
    let header = &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.h")
        .unwrap()
        .contents;
    header
        .lines()
        .find(|line| {
            line.starts_with("void emela_embedded_release_value_")
                && line.contains(&format!("(emela_type_{ty} *value)"))
        })
        .unwrap()
        .strip_prefix("void ")
        .unwrap()
        .split('(')
        .next()
        .unwrap()
        .to_owned()
}

#[test]
fn string_aggregates_own_borrowed_inputs_and_native_results_across_suspend_and_cancellation() {
    let (output, ty, _) = fixture();
    let release = release_helper(&output, ty);
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
void test_cancel(EmelaFiber h);
static EmelaTask task;
static int waits;
uint32_t emela_board_event_lock(void) {{ return 0; }}
void emela_board_event_unlock(uint32_t p) {{ (void)p; }}
void board_wait_start(EmelaTask t, int32_t n) {{ assert(n == ++waits); task = t; }}
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ fprintf(stderr, "%d: %.*s\n", k,n,m); abort(); }}
int main(void) {{
  for (int i = 0; i < 1000; ++i) {{
    emela_type_{ty} native = emela_export_make_0(INT32_MIN);
    assert(native.f0.owner && native.f1.payload.v0.f0.owner);
    assert(native.f0.length == 11 && memcmp(native.f0.data, "-2147483648", 11) == 0);
    {release}(&native);
    assert(test_heap_empty());
    char left[] = {{'a', 0, 'b'}}, right[] = "xyz";
    emela_type_{ty} input = {{.f0 = {{.data = left, .length = 3}},
      .f1 = {{.tag = 0, .payload.v0.f0 = {{.data = right, .length = 3}}}}}};
    emela_type_{ty} result = {{0}}; waits = 0;
    int mode = i % 6;
    if (mode == 5) input = emela_export_make_0(42);
    void *owner0 = input.f0.owner, *owner1 = input.f1.payload.v0.f0.owner;
    EmelaFiber f = emela_export_delayed_1_start(input, mode != 1, mode == 4 ? NULL : &result);
    if (mode == 5) {release}(&input);
    memset(left, 'x', 3); memset(right, 'x', 3);
    while (emela_scheduler_run(1)) {{}} assert(waits == 1);
    if (mode == 2) test_cancel(f);
    else {{
      assert(emela_task_complete_int(task, 0));
      while (emela_scheduler_run(1)) {{}} assert(waits == 2);
      if (mode == 3) test_cancel(f);
      else assert(emela_task_complete_int(task, 0));
    }}
    while (!emela_fiber_done(f)) emela_scheduler_run(1);
    emela_fiber_release(f);
    assert(!emela_task_complete_int(task, 0));
    if (mode < 2 || mode == 5) {{
      assert(result.f0.owner);
      if (mode == 0) {{
        assert(result.f0.length == 3 && memcmp(result.f0.data, "a\0b", 3) == 0);
        assert(result.f1.tag == 0 && result.f1.payload.v0.f0.owner);
        assert(result.f1.payload.v0.f0.length == 3 && memcmp(result.f1.payload.v0.f0.data, "xyz", 3) == 0);
      }} else if (mode == 1) assert(result.f0.length == 4 && memcmp(result.f0.data, "none", 4) == 0 && result.f1.tag == 1);
      else {{
        assert(result.f0.owner == owner0 && result.f1.payload.v0.f0.owner == owner1);
        assert(result.f0.length == 2 && memcmp(result.f0.data, "42", 2) == 0);
      }}
      {release}(&result);
    }} else assert(!result.f0.owner);
    assert(test_heap_empty());
  }}
}}
"#
        ),
    );
}

#[test]
fn native_closure_captures_string_tuple_enum_and_list_after_factory_stack_returns() {
    let (output, _, callback) = fixture();
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
uint32_t emela_board_event_lock(void) {{ return 0; }}
void emela_board_event_unlock(uint32_t p) {{ (void)p; }}
void board_wait_start(EmelaTask t, int32_t n) {{ (void)t; (void)n; abort(); }}
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ fprintf(stderr, "%d: %.*s\n", k,n,m); abort(); }}
int main(void) {{
  for (int i = 0; i < 1000; ++i) {{
    char bytes[] = "abc";
    emela_function_type_{callback} f = emela_export_maker_2((EmelaString){{.data = bytes, .length = 3}}, 42);
    memset(bytes, 'x', 3);
    emela_embedded_retain_function_{callback}(&f);
    emela_function_type_{callback} copy = f;
    emela_embedded_release_function_{callback}(&f);
    EmelaString value = copy.call(copy.env);
    emela_embedded_release_function_{callback}(&copy);
    assert(value.owner && value.length == 11 && memcmp(value.data, "abc42abc/43", 11) == 0);
    emela_embedded_release_string(&value);
    assert(test_heap_empty());
  }}
}}
"#
        ),
    );
}

#[test]
fn suspended_child_captures_and_scope_results_keep_string_aggregates_until_last_release() {
    let (output, ty, _) = fixture();
    let release = release_helper(&output, ty);
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
static EmelaTask tasks[2];
static int count;
uint32_t emela_board_event_lock(void) {{ return 0; }}
void emela_board_event_unlock(uint32_t p) {{ (void)p; }}
void board_wait_start(EmelaTask t, int32_t n) {{ assert(n >= 1 && n <= 2); tasks[n-1] = t; count++; }}
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ fprintf(stderr, "%d: %.*s\n", k,n,m); abort(); }}
int main(void) {{
  for (int i = 0; i < 1000; ++i) {{
    emela_type_{ty} result = {{0}}; count = 0;
    int cancel = i % 2, discard = i % 3 == 0;
    EmelaFiber f = emela_export_scoped_3_start(42, cancel, discard ? NULL : &result);
    while (emela_scheduler_run(1)) {{}} assert(count == 2);
    assert(emela_task_complete_int(tasks[1], 0));
    while (emela_scheduler_run(1)) {{}}
    if (!cancel) {{ assert(!emela_fiber_done(f)); assert(emela_task_complete_int(tasks[0], 0)); }}
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
    assert(!emela_task_complete_int(tasks[0], 0));
    if (!discard) {{
      assert(result.f0.owner && result.f0.length == 4);
      assert(memcmp(result.f0.data, cancel ? "none" : "4244", 4) == 0);
      if (cancel) assert(result.f1.tag == 1);
      else assert(result.f1.tag == 0 && result.f1.payload.v0.f0.owner && result.f1.payload.v0.f0.length == 2 && memcmp(result.f1.payload.v0.f0.data, "43", 2) == 0);
      {release}(&result);
    }}
    assert(test_heap_empty());
  }}
}}
"#
        ),
    );
}
