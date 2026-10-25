use super::*;

fn append_source(output: &mut Output, text: &str) {
    output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents
        .push_str(text);
}

fn release_helper(output: &Output, ty: TypeId) -> String {
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
                && line.contains(&format!("(emela_type_{} *value)", ty.index()))
        })
        .unwrap()
        .strip_prefix("void ")
        .unwrap()
        .split('(')
        .next()
        .unwrap()
        .to_owned()
}

fn add_heap_probe(output: &mut Output) {
    append_source(
        output,
        r#"
size_t test_heap_used(void) {
  size_t used = 0;
  if (!emela_heap_initialized) return 0;
  for (unsigned char *at = emela_heap_storage; at < emela_heap_end();) {
    emela_heap_block *block = (emela_heap_block *)(void *)at;
    if (!block->is_free) used += block->size;
    at += block->size;
  }
  return used;
}
size_t test_owner_capacity(void *owner) {
  return emela_heap_header(owner)->size - emela_heap_header_size();
}
"#,
    );
}

#[test]
fn returned_list_owns_formatted_strings_after_the_producer_stack_returns() {
    let core = support::try_lower(
        &[(
            "main.emel",
            include_str!("../../examples/support/owned_payload.emel"),
        )],
        Roots::Public,
    )
    .unwrap();
    let mut output = compile(&core, &Options::default().with_heap_bytes(1024)).unwrap();
    add_heap_probe(&mut output);
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
size_t test_heap_used(void);
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
int main(void) {
  for (int i = 0; i < 1000; ++i) {
    void *list = emela_export_store_0(INT32_MIN);
    EmelaString value = emela_export_read_1(list);
    emela_embedded_release(list);
    assert(value.owner && value.length == 11 && memcmp(value.data, "-2147483648", 11) == 0);
    emela_embedded_release_string(&value);
    assert(test_heap_used() == 0);
  }
}
"#,
    );
}

#[test]
fn heap_payload_promotion_owns_inline_strings_and_shares_existing_backing() {
    let mut core = emela_core::Module::new();
    let maybe = core.add_enum(EnumDef {
        name: "MaybeText".into(),
        params: 0,
        variants: vec![
            VariantDef::positional("Some", vec![TypeId::STRING]),
            VariantDef::unit("None"),
        ],
        error: false,
        external: false,
    });
    let maybe_ty = core.enum_type(maybe, vec![]);
    let tuple_ty = core.tuple(vec![TypeId::STRING, maybe_ty]);
    let list_ty = core.list(tuple_ty);
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "store");
    b.set_ret(list_ty).set_exported();
    let text = b.param("text", TypeId::STRING);
    let some = b.assign(
        "some",
        maybe_ty,
        Value::Make {
            ctor: CtorRef {
                enum_id: maybe,
                variant: 0,
            },
            args: vec![text.into()],
        },
    );
    let tuple = b.assign(
        "tuple",
        tuple_ty,
        Value::Tuple(vec![text.into(), some.into()]),
    );
    let list = b.assign(
        "list",
        list_ty,
        Value::List {
            elems: vec![tuple.into()],
            tail: None,
        },
    );
    b.term(Term::Return(Some(list.into())));
    flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "head");
    b.set_ret(tuple_ty).set_exported();
    let list = b.param("list", list_ty);
    let tuple = b.assign("tuple", tuple_ty, Value::ListHead(list));
    b.term(Term::Return(Some(tuple.into())));
    flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "pack");
    b.set_ret(TypeId::ANY).set_exported();
    let tuple = b.param("tuple", tuple_ty);
    let any = b.assign("any", TypeId::ANY, Value::Coerce(tuple));
    b.term(Term::Return(Some(any.into())));
    flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "unpack");
    b.set_ret(tuple_ty).set_exported();
    let any = b.param("any", TypeId::ANY);
    let tuple = b.assign("tuple", tuple_ty, Value::Coerce(any));
    b.term(Term::Return(Some(tuple.into())));
    flow.add(b.finish());
    let mut output = compile_flow(&flow, &Options::default().with_heap_bytes(1024)).unwrap();
    let release = release_helper(&output, tuple_ty);
    add_heap_probe(&mut output);
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
size_t test_heap_used(void);
size_t test_owner_capacity(void *owner);
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ (void)k; (void)m; (void)n; abort(); }}
int main(void) {{
  for (int i = 0; i < 1000; ++i) {{
    char bytes[] = {{'a', 0, 'b', (char)0xc3, (char)0xa9}};
    EmelaString input = {{.data = bytes, .length = sizeof bytes}};
    void *list = emela_export_store_0(input);
    memset(bytes, 'x', sizeof bytes);
    emela_type_{ty} tuple = emela_export_head_1(list);
    emela_embedded_release(list);
    assert(tuple.f0.owner && tuple.f1.payload.v0.f0.owner);
    assert(tuple.f0.length == 5 && memcmp(tuple.f0.data, "a\0b\xc3\xa9", 5) == 0);
    assert(test_owner_capacity(tuple.f0.owner) < 64);
    EmelaAny any = emela_export_pack_2(tuple);
    emela_type_{ty} copy = emela_export_unpack_3(any);
    assert(copy.f0.owner == tuple.f0.owner && copy.f1.payload.v0.f0.owner == tuple.f1.payload.v0.f0.owner);
    {release}(&tuple);
    emela_embedded_release(any.payload.any_box);
    assert(memcmp(copy.f1.payload.v0.f0.data, "a\0b\xc3\xa9", 5) == 0);
    {release}(&copy);
    assert(test_heap_used() == 0);

    tuple = (emela_type_{ty}){{.f0 = input, .f1 = {{.tag = 1}}}};
    any = emela_export_pack_2(tuple);
    copy = emela_export_unpack_3(any);
    assert(copy.f1.tag == 1 && copy.f0.owner);
    emela_embedded_release(any.payload.any_box);
    {release}(&copy);
    assert(test_heap_used() == 0);
  }}
}}
"#,
            ty = tuple_ty.index()
        ),
    );
}

#[test]
fn recursive_heap_payload_promotion_and_destruction_do_not_follow_value_depth_on_stack() {
    let mut core = emela_core::Module::new();
    let chain = core.add_enum(EnumDef {
        name: "TextChain".into(),
        params: 0,
        variants: vec![],
        error: false,
        external: false,
    });
    let chain_ty = core.enum_type(chain, vec![]);
    core.enums[chain].variants = vec![
        VariantDef::unit("End"),
        VariantDef::positional("Link", vec![TypeId::STRING, chain_ty]),
    ];
    let list_ty = core.list(chain_ty);
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "link");
    b.set_ret(chain_ty).set_exported();
    let text = b.param("text", TypeId::STRING);
    let tail = b.param("tail", chain_ty);
    let value = b.assign(
        "value",
        chain_ty,
        Value::Make {
            ctor: CtorRef {
                enum_id: chain,
                variant: 1,
            },
            args: vec![text.into(), tail.into()],
        },
    );
    b.term(Term::Return(Some(value.into())));
    flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "hold");
    b.set_ret(list_ty).set_exported();
    let value = b.param("value", chain_ty);
    let list = b.assign(
        "list",
        list_ty,
        Value::List {
            elems: vec![value.into()],
            tail: None,
        },
    );
    b.term(Term::Return(Some(list.into())));
    flow.add(b.finish());
    let mut output = compile_flow(&flow, &Options::default().with_heap_bytes(700_000)).unwrap();
    let release = release_helper(&output, chain_ty);
    add_heap_probe(&mut output);
    append_source(
        &mut output,
        &format!(
            r#"
#include <assert.h>
#include <string.h>
EmelaString test_hold_text(void *list) {{
  return emela_heap_own_string(((emela_list_node_0 *)list)->head.payload.v1.f0);
}}
void test_chain(void *list, int count) {{
  const emela_type_{ty} *value = &((emela_list_node_0 *)list)->head;
  for (int i = 0; i < count; ++i) {{
    assert(value->tag == 1);
    assert(value->payload.v1.f0.owner && value->payload.v1.f0.length == 3);
    assert(memcmp(value->payload.v1.f0.data, "abc", 3) == 0);
    value = (const emela_type_{ty} *)value->payload.v1.f1;
  }}
  assert(value->tag == 0);
}}
"#,
            ty = chain_ty.index()
        ),
    );
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include "emela_embedded.h"
#include "emela_board.h"
size_t test_heap_used(void);
EmelaString test_hold_text(void *list);
void test_chain(void *list, int count);
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ (void)k; (void)m; (void)n; abort(); }}
int main(void) {{
  struct rlimit limit = {{65536, 65536}};
  assert(setrlimit(RLIMIT_STACK, &limit) == 0);
  for (int run = 0; run < 10; ++run) {{
    char bytes[] = "abc";
    emela_type_{ty} end = {{.tag = 0}};
    emela_type_{ty} first = emela_export_link_0((EmelaString){{.data = bytes, .length = 3}}, end);
    emela_type_{ty} second = emela_export_link_0((EmelaString){{.data = bytes, .length = 3}}, first);
    void *borrowed = emela_export_hold_1(second);
    {release}(&first);
    {release}(&second);
    memset(bytes, 'x', 3);
    test_chain(borrowed, 2);
    emela_embedded_release(borrowed);
    assert(test_heap_used() == 0);
    memcpy(bytes, "abc", 3);
    first = emela_export_link_0((EmelaString){{.data = bytes, .length = 3}}, end);
    void *seed = emela_export_hold_1(first);
    EmelaString stable = test_hold_text(seed);
    {release}(&first);
    emela_embedded_release(seed);
    emela_type_{ty} value = end;
    for (int i = 0; i < 6000; ++i) {{
      emela_type_{ty} next = emela_export_link_0(stable, value);
      {release}(&value);
      value = next;
    }}
    void *list = emela_export_hold_1(value);
    {release}(&value);
    emela_embedded_release_string(&stable);
    memset(bytes, 'x', 3);
    test_chain(list, 6000);
    emela_embedded_release(list);
    assert(test_heap_used() == 0);
  }}
}}
"#,
            ty = chain_ty.index()
        ),
    );
}

#[test]
fn promoted_any_payload_is_reclaimed_when_its_waiting_frame_is_cancelled() {
    let mut core = emela_core::Module::new();
    let tuple_ty = core.tuple(vec![TypeId::STRING]);
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "store");
    b.set_exported().set_ret(TypeId::ANY);
    let text = b.param("text", TypeId::STRING);
    let tuple = b.assign("tuple", tuple_ty, Value::Tuple(vec![text.into()]));
    let any = b.assign("any", TypeId::ANY, Value::Coerce(tuple));
    b.term(Term::Return(Some(any.into())));
    flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "wait");
    b.set_suspends().set_ret(TypeId::INT);
    b.param("n", TypeId::INT);
    let wait = flow.add(b.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("host_wait".into()),
        }],
    }));
    let mut b = FnBuilder::new(&flow.core, "keep");
    b.set_exported().set_suspends().set_ret(TypeId::ANY);
    let any = b.param("any", TypeId::ANY);
    b.protect(
        None,
        |b| {
            b.assign(
                "waited",
                TypeId::INT,
                Value::Call {
                    callee: Callee::Direct(wait),
                    args: vec![Const::Int(1).into()],
                    suspends: true,
                    on_fail: None,
                },
            );
            b.term(Term::Yield(None));
        },
        |b| b.term(Term::Yield(None)),
    );
    b.term(Term::Return(Some(any.into())));
    flow.add(b.finish());
    let mut output = compile_flow(
        &flow,
        &Options::default()
            .with_heap_bytes(1024)
            .with_fiber_capacity(1)
            .with_frame_capacity(3)
            .with_frame_bytes(512)
            .with_task_capacity(1)
            .with_event_capacity(1)
            .with_task_adapter("host_wait", "board_wait_start"),
    )
    .unwrap();
    add_heap_probe(&mut output);
    // Exercise the internal cancellation boundary without adding a public cancellation API.
    append_source(
        &mut output,
        "void test_cancel(EmelaFiber handle) { emela_fiber_get(handle)->cancelled = 1; }\n",
    );
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask task;
static int started;
size_t test_heap_used(void);
void test_cancel(EmelaFiber handle);
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
void board_wait_start(EmelaTask t, int32_t n) { assert(n == 1); task = t; started = 1; }
int main(void) {
  for (int i = 0; i < 1000; ++i) {
    char bytes[] = "abc";
    EmelaAny value = emela_export_store_0((EmelaString){.data = bytes, .length = 3});
    EmelaAny result = {0}; started = 0;
    EmelaFiber fiber = emela_export_keep_2_start(value, &result);
    emela_embedded_release(value.payload.any_box);
    memset(bytes, 'x', 3);
    while (!started) assert(emela_scheduler_run(1));
    if (i % 2) test_cancel(fiber);
    else assert(emela_task_complete_int(task, 1));
    while (!emela_fiber_done(fiber)) emela_scheduler_run(1);
    emela_fiber_release(fiber);
    if (i % 2) assert(!result.payload.any_box);
    else { assert(result.payload.any_box); emela_embedded_release(result.payload.any_box); }
    assert(!emela_task_complete_int(task, 1));
    assert(test_heap_used() == 0);
  }
}
"#,
    );
}
