use super::*;

const BASE_C: &str = r#"
#include <assert.h>
#include <limits.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
static void equal_text(EmelaString text, const char *expected) {
  size_t length = strlen(expected);
  if (text.length != length || memcmp(text.data, expected, length)) {
    fprintf(stderr, "got %.*s (%u), expected %s (%zu)\n", text.length, text.data, text.length, expected, length);
    abort();
  }
}
"#;
const ABORT_DEFECT: &str = r#"
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *text, uint16_t length) {
  fprintf(stderr, "defect %d: %.*s\n", kind, length, text);
  abort();
}
"#;
const IGNORE_DEBUG: &str = r#"
void emela_board_debug(const char *text, uint16_t length) { (void)text; (void)length; }
"#;

fn fixture(source: &str) -> Output {
    let core = support::try_lower(&[("main.emel", source)], Roots::Public).unwrap();
    let flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    assert_eq!(flow.stage, emela_lir::Stage::Flow);
    emela_codegen_emb::verify_flow(&flow).unwrap();
    for (_, function) in flow.functions.iter() {
        for (_, statement) in function.stmts.iter() {
            if let emela_lir::Stmt::Let {
                value: Value::Builtin { op, .. },
                ..
            } = statement
            {
                assert!(!matches!(
                    op,
                    Builtin::ShowNested
                        | Builtin::Inspect
                        | Builtin::ShowString
                        | Builtin::DebugPrint
                ));
            }
        }
    }
    let mut output = compile_flow(
        &flow,
        &Options::default()
            .with_heap_bytes(16384)
            .with_string_capacity(256),
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
int test_heap_empty(void) {
  if (!emela_heap_initialized) return 1;
  for (unsigned char *p = emela_heap_storage; p < emela_heap_end();) {
    emela_heap_block *block = (emela_heap_block *)(void *)p;
    if (!block->is_free) return 0;
    p += block->size;
  }
  return 1;
}
"#,
        );
    output
}

fn exported(output: &Output, stem: &str) -> String {
    let prefix = format!("emela_export_{stem}_");
    let header = &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.h")
        .unwrap()
        .contents;
    let declaration = header.lines().find(|line| line.contains(&prefix)).unwrap();
    declaration[declaration.find(&prefix).unwrap()..]
        .split('(')
        .next()
        .unwrap()
        .to_owned()
}

#[test]
fn derived_and_written_traits_keep_static_dispatch_and_escaped_display() {
    let output = fixture(include_str!("traits.emel"));
    let traits = exported(&output, "traits");
    let quoted = exported(&output, "quoted");
    compile_and_run(
        output,
        &format!(
            r#"
{BASE_C}
{ABORT_DEFECT}
{IGNORE_DEBUG}
int main(void) {{
  char buffer[256];
  static const char bytes[] = {{'a', 0, '"', '\\', '\n', '#', '{{', (char)0xce, (char)0xbb}};
  EmelaString text = {{bytes, sizeof bytes, NULL}};
  for (unsigned i = 0; i < 1000; ++i) {{
    equal_text({traits}(7, (EmelaString){{"x", 1, NULL}}, buffer, sizeof buffer), "(Envelope(key: K7, text: \"x\"), [Some(K7)], Present(K7, \"x\"))");
    equal_text({quoted}(text, buffer, sizeof buffer), "\"a\\u{{0}}\\\"\\\\\\n\\#{{\xce\xbb\"");
    assert(test_heap_empty());
  }}
}}
"#
        ),
    );
}

#[test]
fn debug_and_error_display_respect_written_derived_and_missing_show() {
    let output = fixture(include_str!("traits.emel"));
    let debugging = exported(&output, "debugging");
    let written = exported(&output, "written_cause");
    let derived = exported(&output, "derived_cause");
    let plain = exported(&output, "plain_cause");
    compile_and_run(
        output,
        &format!(
            r#"
{BASE_C}
{ABORT_DEFECT}
static unsigned debug_count;
void emela_board_debug(const char *text, uint16_t length) {{
  static const char *expected[] = {{"<fn>", "<fn>", "K7"}};
  assert(debug_count < 3);
  equal_text((EmelaString){{text, length, NULL}}, expected[debug_count++]);
}}
int main(void) {{
  char buffer[256]; EmelaString text = {{"x", 1, NULL}};
  for (unsigned i = 0; i < 1000; ++i) {{
    debug_count = 0;
    assert({debugging}(7) == 7 && debug_count == 3);
    equal_text({written}(text, buffer, sizeof buffer), "error: written<x>");
    equal_text({derived}(text, buffer, sizeof buffer), "error: Derived(message: \"x\")");
    equal_text({plain}(text, buffer, sizeof buffer), "error: Plain");
    assert(test_heap_empty());
  }}
}}
"#
        ),
    );
}

#[test]
fn invalid_expanding_function_equality_is_rejected_before_c_emission() {
    let mut core = emela_core::Module::new();
    let parameter = core.type_param(0);
    let callback = core.fn_type(vec![TypeId::INT], TypeId::INT);
    let family = core.add_enum(emela_core::EnumDef {
        name: "GrowFn".into(),
        params: 1,
        variants: vec![],
        error: false,
        external: false,
    });
    let list = core.list(parameter);
    let next = core.enum_type(family, vec![list]);
    core.enums[family].variants = vec![
        emela_core::VariantDef::positional("EmptyFn", vec![]),
        emela_core::VariantDef::positional("EndFn", vec![parameter, callback]),
        emela_core::VariantDef::positional("NextFn", vec![next]),
    ];
    let ty = core.enum_type(family, vec![TypeId::INT]);
    let mut invalid = Module::new(core);
    let mut eq = FnBuilder::new(&invalid.core, "invalid_fn_equality");
    eq.param("left", ty);
    eq.param("right", ty);
    eq.set_ret(TypeId::BOOL);
    let eq = invalid.add(eq.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "emela-emb-derived".into(),
            host: HostRef::Global("eq".into()),
        }],
    }));
    let mut caller = FnBuilder::new(&invalid.core, "probe");
    caller.set_exported().set_ret(TypeId::BOOL);
    let left = caller.param("left", ty);
    let right = caller.param("right", ty);
    let result = caller.assign(
        "result",
        TypeId::BOOL,
        Value::Call {
            callee: Callee::Direct(eq),
            args: vec![left.into(), right.into()],
            suspends: false,
            on_fail: None,
        },
    );
    caller.term(Term::Return(Some(result.into())));
    invalid.add(caller.finish());
    emela_codegen_emb::verify_flow(&invalid).unwrap();
    let errors = compile_flow(&invalid, &Options::default().with_heap_bytes(2048))
        .expect_err("functions have no structural equality");
    assert!(errors.iter().any(|e| e.kind == ErrorKind::Unsupported
        && e.message.contains("structural derivation signature")));
}

#[test]
fn public_failure_entry_keeps_the_embedded_immediate_defect_contract() {
    let output = fixture(include_str!("traits.emel"));
    let entry = exported(&output, "entry_failure");
    compile_and_run(
        output,
        &format!(
            r#"
{BASE_C}
{IGNORE_DEBUG}
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *text, uint16_t length) {{
  assert(kind == EMELA_DEFECT_UNHANDLED_ERROR);
  equal_text((EmelaString){{text, length, NULL}}, "unhandled error");
  exit(0);
}}
int main(void) {{ (void){entry}((EmelaString){{"x", 1, NULL}}); return 1; }}
"#
        ),
    );
}

#[test]
fn ordered_maps_sets_and_rotations_reclaim_persistent_values() {
    let output = fixture(include_str!("collections.emel"));
    let collections = exported(&output, "collections");
    let rotations = exported(&output, "rotations");
    compile_and_run(
        output,
        &format!(
            r#"
{BASE_C}
{ABORT_DEFECT}
int main(void) {{
  (void)equal_text;
  for (int i = 0; i < 1000; ++i) {{
    assert({collections}(i) == 3 * i + 41);
    assert(test_heap_empty());
    assert({rotations}(i) == 12 * i);
    assert(test_heap_empty());
  }}
}}
"#
        ),
    );
}

#[test]
fn lambda_patterns_and_new_core_helpers_keep_dynamic_semantics() {
    let output = fixture(include_str!("helpers.emel"));
    let patterns = exported(&output, "lambda_patterns");
    let lists = exported(&output, "list_helpers");
    let ascii = exported(&output, "ascii");
    let numeric = exported(&output, "numeric");
    let numeric64 = exported(&output, "numeric64");
    let number = exported(&output, "numeric_float");
    let boolean = exported(&output, "boolean");
    compile_and_run(
        output,
        &format!(
            r#"
{BASE_C}
{ABORT_DEFECT}
int main(void) {{
  char buffer[256]; static const char text[] = "A\xce\xbbz\0Q";
  EmelaString input = {{text, sizeof text - 1, NULL}};
  for (int i = 0; i < 1000; ++i) {{
    assert({patterns}(i) == 4 * i + 3);
    assert({lists}(i) == 3 * i + 3);
    EmelaString result = {ascii}(input, buffer, sizeof buffer);
    static const char expected[] = "a\xce\xbbz\0q|A\xce\xbbZ\0Q";
    assert(result.length == sizeof expected - 1 && !memcmp(result.data, expected, sizeof expected - 1));
    assert({numeric}(-i) == i && {numeric}(INT32_MIN) == INT32_MIN);
    assert({numeric64}(INT64_MIN) == INT64_MIN && {numeric64}(-INT64_MAX) == INT64_MAX);
    assert({number}(NAN) && {number}(-0.0) && {number}(-INFINITY));
    assert({boolean}((EmelaString){{"True", 4, NULL}}));
    assert(!{boolean}((EmelaString){{"False", 5, NULL}}));
    assert(test_heap_empty());
  }}
  (void)equal_text;
}}
"#
        ),
    );
}
