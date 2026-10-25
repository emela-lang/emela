use super::*;
mod external;

// Source fixtures wrap callable const values in tuples so the unchanged Core lowerer keeps
// the const result type intact. This exercises backend caching and callable specialization
// through verified Core without relying on the upstream bare-function const lowering bug.
fn emit_source(source: &str, heap: u32) -> Output {
    let core =
        support::try_lower(&[("main.emel", source)], Roots::Public).expect("lower const source");
    compile(&core, &Options::default().with_heap_bytes(heap)).expect("emit runtime const")
}

fn export(output: &Output, name: &str) -> String {
    let header = &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.h")
        .unwrap()
        .contents;
    let prefix = format!("emela_export_{name}_");
    let start = header.find(&prefix).unwrap();
    header[start..].split('(').next().unwrap().to_owned()
}

const BOARD: &str = r#"
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
  (void)kind; (void)message; (void)length; abort();
}
"#;

#[test]
fn evaluates_branch_loop_recursive_helper_and_closure_const_once() {
    let output = emit_source(
        r#"
fn sum(n: Int) -> Int { if n == 0 { 0 } else { n + sum(n - 1) } }
fn loop_sum(n: Int, acc: Int) -> Int { if n == 0 { acc } else { loop_sum(n - 1, acc + n) } }
const VIA_INDIRECT = { f = sum
 f(6) }
const VALUE = { dbg("once")
 if True { VIA_INDIRECT + loop_sum(6, 0) - 21 } else { 0 } }
const APPLY = { x = VALUE
 (fn(y: Int) { x + y }, ()) }
pub fn read() -> Int { (apply, _) = APPLY
 apply(VALUE) }
"#,
        4096,
    );
    let read = export(&output, "read");
    compile_and_run(
        output,
        &format!(
            r#"{BOARD}
static int debug_calls;
void emela_board_debug(const char *bytes, uint16_t length) {{ (void)bytes; (void)length; debug_calls++; }}
int main(void) {{ for (volatile int i = 0; i < 1000; i++) if ({read}() != 42) return 1; return debug_calls == 1 ? 0 : 2; }}
"#
        ),
    );
}

#[test]
fn keeps_list_tuple_enum_and_option_string_const_values_alive() {
    let output = emit_source(
        r#"
const VALUE = { dbg("once")
 (Some(42), ["a", "bc"], String.from_code_point(65)) }
pub fn read() -> Int {
  (parsed, values, point) = VALUE
  n = parsed match { Some(n) -> n
_ -> -100 }
  first = values match { [a, ..rest] -> String.byte_size(a)
_ -> 0 }
  size = point match { Some(s) -> String.byte_size(s)
_ -> 0 }
  n + first + size
}
"#,
        8192,
    );
    let read = export(&output, "read");
    compile_and_run(
        output,
        &format!(
            r#"{BOARD}
static int debug_calls;
void emela_board_debug(const char *bytes, uint16_t length) {{ (void)bytes; (void)length; debug_calls++; }}
int main(void) {{ for (volatile int i = 0; i < 1000; i++) if ({read}() != 44) return 1; return debug_calls == 1 ? 0 : 2; }}
"#
        ),
    );
}

#[test]
fn caches_runtime_const_external_results_and_discards_without_reinitializing() {
    let output = emit_source(
        r#"
@external(c, "fixture_seed")
fn raw_seed() -> Int
effect Seed { fn read() -> Int }
handler BoardSeed: Seed { fn read() { raw_seed() } }
fn seed() -> Int { with BoardSeed { (use Seed).read() } }
const VALUE = seed()
const UNUSED = seed()
pub fn read() -> Int { VALUE }
pub fn discard() { VALUE
() }
"#,
        8192,
    );
    let read = export(&output, "read");
    let discard = export(&output, "discard");
    compile_and_run(
        output,
        &format!(
            r#"{BOARD}
static int calls;
int32_t fixture_seed(void) {{ return ++calls + 41; }}
int main(void) {{ {discard}(); for (volatile int i = 0; i < 1000; i++) if ({read}() != 42) return 1; return calls == 1 ? 0 : 2; }}
"#
        ),
    );
}

#[test]
fn memoizes_nested_const_defects_and_retries_self_reference_without_prefix() {
    let output = emit_source(
        r#"
@external(c, "fixture_zero")
fn raw_zero() -> Int
effect Zero { fn read() -> Int }
handler BoardZero: Zero { fn read() { raw_zero() } }
fn zero() -> Int { with BoardZero { (use Zero).read() } }
const B = 1 / zero()
const A = { dbg("A")
 B }
const SELF = if zero() == 0 { SELF } else { 42 }
pub fn bad() -> Int { A }
pub fn self_ref() -> Int { SELF }
"#,
        8192,
    );
    let bad = export(&output, "bad");
    let self_ref = export(&output, "self_ref");
    compile_and_run(
        output,
        &format!(
            r#"
#include <setjmp.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static jmp_buf target;
static int calls, debug_calls, mode;
int32_t fixture_zero(void) {{ calls++; return 0; }}
void emela_board_debug(const char *bytes, uint16_t length) {{ (void)bytes; (void)length; debug_calls++; }}
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {{
  const char *expected = mode == 0 ? "while evaluating const A: while evaluating const B: division by zero" : "const SELF refers to itself while being evaluated";
  if (length != strlen(expected) || memcmp(message, expected, length) != 0 || (mode == 0 && kind != EMELA_DEFECT_DIVISION_BY_ZERO)) abort();
  longjmp(target, 1);
}}
int main(void) {{
  for (volatile int i = 0; i < 1000; i++) if (setjmp(target) == 0) {{ (void){bad}(); return 1; }}
  if (calls != 1 || debug_calls != 1) return 2;
  mode = 1;
  for (volatile int i = 0; i < 1000; i++) if (setjmp(target) == 0) {{ (void){self_ref}(); return 3; }}
  return calls == 1001 ? 0 : 4;
}}
"#
        ),
    );
}

#[test]
fn rejects_runtime_const_reachable_from_isr_but_preserves_static_fold() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
@external(c, "fixture_seed")
fn raw_seed() -> Int
effect Seed { fn read() -> Int }
handler BoardSeed: Seed { fn read() { raw_seed() } }
const V = with BoardSeed { (use Seed).read() }
pub fn read() -> Int { V }
"#,
        )],
        Roots::Public,
    )
    .unwrap();
    let errors = compile(
        &core,
        &Options::default()
            .with_heap_bytes(8192)
            .with_isr_entry("read")
            .with_isr_safe_external("fixture_seed"),
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("runtime const initialization"))
    );
    let core = support::try_lower(
        &[("main.emel", "const V = 40 + 2\npub fn read() -> Int { V }")],
        Roots::Public,
    )
    .unwrap();
    let output = compile(&core, &Options::default().with_isr_entry("read")).unwrap();
    assert!(
        !output
            .files
            .iter()
            .find(|f| f.path == "emela_embedded.c")
            .unwrap()
            .contents
            .contains("EmelaConstState")
    );
}

fn external_const_flow() -> Module {
    let core = emela_core::Module::new();
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "seed");
    b.set_ret(TypeId::INT);
    let seed = flow.add(b.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("fixture_seed".into()),
        }],
    }));
    let mut b = FnBuilder::new(&flow.core, "VALUE");
    b.set_ret(TypeId::INT).set_constant().set_exported();
    let result = b.assign(
        "result",
        TypeId::INT,
        Value::Call {
            callee: Callee::Direct(seed),
            args: vec![],
            suspends: false,
            on_fail: None,
        },
    );
    b.term(Term::Return(Some(result.into())));
    let value = flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "discard");
    b.set_exported();
    b.let_(
        None,
        Value::Call {
            callee: Callee::Direct(value),
            args: vec![],
            suspends: false,
            on_fail: None,
        },
    );
    b.term(Term::Return(None));
    flow.add(b.finish());
    flow
}

#[test]
fn verified_flow_runtime_scalar_const_uses_no_heap_and_caches_discarded_first_reference() {
    let output = compile_flow(&external_const_flow(), &Options::default()).unwrap();
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
            r#"{BOARD}
static int calls;
int32_t fixture_seed(void) {{ return ++calls + 41; }}
int main(void) {{ emela_export_discard_2(); for (volatile int i=0; i<1000; i++) if (emela_export_VALUE_1() != 42) return 1; return calls == 1 ? 0 : 2; }}
"#
        ),
    );
}

#[test]
fn public_defect_report_snapshots_borrowed_message_once_without_using_heap() {
    let output = compile_flow(&external_const_flow(), &Options::default()).unwrap();
    compile_and_run(
        output,
        r#"
#include <setjmp.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static jmp_buf target;
static int calls;
static char message[] = "borrowed failure";
int32_t fixture_seed(void) { calls++; emela_embedded_report_defect(EMELA_DEFECT_ASSERT, message, sizeof message - 1); }
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *bytes, uint16_t length) {
  static const char expected[] = "while evaluating const VALUE: borrowed failure";
  if (kind != EMELA_DEFECT_ASSERT || length != sizeof expected - 1 || memcmp(bytes, expected, length)) abort();
  memset(message, 'x', sizeof message - 1);
  longjmp(target, 1);
}
int main(void) { for (volatile int i=0; i<1000; i++) if (setjmp(target)==0) { (void)emela_export_VALUE_1(); return 1; } return calls == 1 ? 0 : 2; }
"#,
    );
}

#[test]
fn diagnostic_capacity_exhaustion_reports_active_const_without_allocating_or_recursing() {
    let output = compile_flow(
        &external_const_flow(),
        &Options::default().with_const_diagnostic_bytes(1),
    )
    .unwrap();
    compile_and_run(
        output,
        r#"
#include <setjmp.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static jmp_buf target;
static int calls;
int32_t fixture_seed(void) { calls++; emela_embedded_report_defect(EMELA_DEFECT_ASSERT, "bad", 3); }
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *bytes, uint16_t length) {
  static const char expected[] = "while evaluating const VALUE: const diagnostic capacity exceeded";
  if (kind != EMELA_DEFECT_STRING_CAPACITY || length != sizeof expected - 1 || memcmp(bytes, expected, length)) abort();
  longjmp(target, 1);
}
int main(void) { for (volatile int i=0; i<1000; i++) if (setjmp(target)==0) { (void)emela_export_VALUE_1(); return 1; } return calls == 1000 ? 0 : 2; }
"#,
    );
    assert_eq!(
        compile_flow(
            &external_const_flow(),
            &Options::default().with_const_diagnostic_bytes(0)
        )
        .unwrap_err()[0]
            .kind,
        ErrorKind::InvalidOptions
    );
}

#[test]
fn heap_exhaustion_during_const_initialization_keeps_its_name_and_cached_defect() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
const VALUE = [1, 2, 3]
pub fn read() -> Int { VALUE match { [head, ..tail] -> head
_ -> 0 } }
"#,
        )],
        Roots::Public,
    )
    .unwrap();
    let output = compile(&core, &Options::default().with_heap_bytes(64)).unwrap();
    let read = export(&output, "read");
    compile_and_run(
        output,
        &format!(
            r#"
#include <setjmp.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static jmp_buf target;
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *bytes, uint16_t length) {{
  static const char expected[] = "while evaluating const VALUE: fixed heap exhausted";
  if (kind != EMELA_DEFECT_HEAP_EXHAUSTED || length != sizeof expected - 1 || memcmp(bytes, expected, length)) abort();
  longjmp(target, 1);
}}
int main(void) {{ for (volatile int i=0; i<1000; i++) if (setjmp(target)==0) {{ (void){read}(); return 1; }} return 0; }}
"#
        ),
    );
}

use crate::gc_anchor;

#[test]
fn persistent_const_roots_survive_cycle_collection_and_caller_release() {
    let core = support::try_lower(
        &[(
            "main.emel",
            include_str!("../../examples/support/const_runtime.emel"),
        )],
        Roots::Public,
    )
    .unwrap();
    let mut flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    gc_anchor::add(&mut flow);
    let mut output = compile_flow(&flow, &Options::default().with_heap_bytes(4096)).unwrap();
    let read = export(&output, "read");
    output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents
        .push_str(
            r#"
size_t test_const_heap(void) {
  emela_heap_collect_cycles();
  size_t used = 0;
  for (unsigned char *p = emela_heap_storage; p < emela_heap_end();) {
    emela_heap_block *block = (emela_heap_block *)(void *)p;
    if (!block->is_free) used += block->size;
    p += block->size;
  }
  return used;
}
"#,
        );
    compile_and_run(
        output,
        &format!(
            r#"{BOARD}
static int debug_calls;
void emela_board_debug(const char *bytes, uint16_t length) {{ (void)bytes; (void)length; debug_calls++; }}
size_t test_const_heap(void);
int main(void) {{ if ({read}()!=42) return 1; size_t baseline=test_const_heap(); if (!baseline) return 2; for (volatile int i=0; i<1000; i++) if ({read}()!=42 || test_const_heap()!=baseline) return 3; return debug_calls==1 ? 0 : 4; }}
"#
        ),
    );
}

#[test]
fn deep_const_contexts_share_cached_diagnostic_suffixes_and_bound_static_folding() {
    let mut flow = external_const_flow();
    flow.functions[emela_lir::FnId::from_raw(1.into())].exported = false;
    let ids: Vec<_> = (0..80).map(|i| flow.declare(&format!("C{i}"))).collect();
    for (i, &id) in ids.iter().enumerate() {
        let target = ids
            .get(i + 1)
            .copied()
            .unwrap_or(emela_lir::FnId::from_raw(0.into()));
        let mut b = FnBuilder::new(&flow.core, &format!("C{i}"));
        b.set_ret(TypeId::INT).set_constant();
        if i == 0 || i == 79 {
            b.set_exported();
        }
        let value = b.assign(
            "value",
            TypeId::INT,
            Value::Call {
                callee: Callee::Direct(target),
                args: vec![],
                suspends: false,
                on_fail: None,
            },
        );
        b.term(Term::Return(Some(value.into())));
        flow.define(id, b.finish());
    }
    let output =
        compile_flow(&flow, &Options::default().with_const_diagnostic_bytes(8192)).unwrap();
    let outer = export(&output, "C0");
    let inner = export(&output, "C79");
    let expected: String = (0..80)
        .map(|i| format!("while evaluating const C{i}: "))
        .collect::<String>()
        + "bad";
    compile_and_run(
        output,
        &format!(
            r#"
#include <setjmp.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static jmp_buf target;
static int calls, mode;
int32_t fixture_seed(void) {{ calls++; emela_embedded_report_defect(EMELA_DEFECT_PANIC, "bad", 3); }}
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *bytes, uint16_t length) {{
  const char *expected = mode == 0 ? "{expected}" : "while evaluating const C79: bad";
  if (kind != EMELA_DEFECT_PANIC || length != strlen(expected) || memcmp(bytes, expected, length)) abort();
  longjmp(target, 1);
}}
int main(void) {{
  for (volatile int i=0; i<1000; i++) if (setjmp(target)==0) {{ (void){outer}(); return 1; }}
  mode=1;
  for (volatile int i=0; i<1000; i++) if (setjmp(target)==0) {{ (void){inner}(); return 2; }}
  return calls == 1 ? 0 : 3;
}}
"#
        ),
    );
}

#[test]
fn folded_self_reference_with_long_name_does_not_copy_into_short_runtime_emergency_storage() {
    let mut flow = external_const_flow();
    let name = "LONG".repeat(1024);
    let id = flow.declare(&name);
    let mut b = FnBuilder::new(&flow.core, &name);
    b.set_ret(TypeId::INT).set_constant().set_exported();
    let value = b.assign(
        "value",
        TypeId::INT,
        Value::Call {
            callee: Callee::Direct(id),
            args: vec![],
            suspends: false,
            on_fail: None,
        },
    );
    b.term(Term::Return(Some(value.into())));
    flow.define(id, b.finish());
    let output = compile_flow(&flow, &Options::default()).unwrap();
    let call = export(&output, &name);
    compile_and_run(
        output,
        &format!(
            r#"
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int32_t fixture_seed(void) {{ return 42; }}
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *bytes, uint16_t length) {{
  static const char expected[]="const {name} refers to itself while being evaluated";
  exit(kind==EMELA_DEFECT_PANIC && length==sizeof expected-1 && !memcmp(bytes,expected,length) ? 0 : 1);
}}
int main(void) {{ (void){call}(); return 2; }}
"#
        ),
    );
}

#[test]
fn unit_and_never_const_cache_states_do_not_require_a_represented_return_value() {
    let mut flow = Module::new(emela_core::Module::new());
    let b = FnBuilder::new(&flow.core, "tick");
    let tick = flow.add(b.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("fixture_tick".into()),
        }],
    }));
    for (name, ret) in [("ONCE", TypeId::UNIT), ("BROKEN", TypeId::NEVER)] {
        let mut b = FnBuilder::new(&flow.core, name);
        b.set_ret(ret).set_constant().set_exported();
        b.let_(
            None,
            Value::Call {
                callee: Callee::Direct(tick),
                args: vec![],
                suspends: false,
                on_fail: None,
            },
        );
        b.term(if ret == TypeId::UNIT {
            Term::Return(None)
        } else {
            Term::Defect(Const::String("boom".into()).into(), None)
        });
        flow.add(b.finish());
    }
    let output = compile_flow(&flow, &Options::default()).unwrap();
    compile_and_run(
        output,
        r#"
#include <setjmp.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static jmp_buf target;
static int calls;
void fixture_tick(void) { calls++; }
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *bytes, uint16_t length) {
  static const char expected[]="while evaluating const BROKEN: boom";
  if(kind!=EMELA_DEFECT_PANIC || length!=sizeof expected-1 || memcmp(bytes,expected,length)) abort();
  longjmp(target,1);
}
int main(void) {
  for (volatile int i=0;i<1000;i++) emela_export_ONCE_1();
  if(calls!=1) return 1;
  for (volatile int i=0;i<1000;i++) if(setjmp(target)==0) { emela_export_BROKEN_2(); return 2; }
  return calls==2 ? 0 : 3;
}
"#,
    );
}

#[test]
fn source_polymorphic_const_origins_keep_a_canonical_value_and_typed_members_in_verified_flow() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
const EMPTY = { dbg("once")
 [] }
fn ints(xs: List[Int]) -> Int { List.length(xs) }
fn strings(xs: List[String]) -> Int { List.length(xs) }
pub fn read() -> Int { ints(EMPTY) + strings(EMPTY) }
"#,
        )],
        Roots::Public,
    )
    .unwrap();
    let flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    emela_lir::verify(&flow).unwrap();
    let (_, marker) = flow
        .functions
        .iter()
        .find(|(_, f)| f.name.starts_with("$emb_const_origins$v1$"))
        .expect("origin metadata survives the public Flow result");
    assert!(marker.constant && !marker.exported && marker.params.is_empty());
    let seq = &marker.seqs[marker.body.unwrap()];
    let targets: Vec<_> = seq
        .stmts
        .iter()
        .map(|&id| {
            let emela_lir::Stmt::Let {
                value: Value::FnRef(target),
                ..
            } = marker.stmts[id]
            else {
                panic!("origin marker contains only typed function references")
            };
            target
        })
        .collect();
    assert_eq!(targets.len(), 3);
    assert_eq!(flow.functions[targets[0]].name, "EMPTY");
    assert!(matches!(
        flow.core.types[flow.functions[targets[0]].ret],
        Type::List(TypeId::ANY)
    ));
    for &target in &targets[1..] {
        assert!(flow.functions[target].name.starts_with("EMPTY["));
    }
}

#[test]
fn malformed_reserved_const_origin_markers_are_invalid_flow() {
    let mut flow = external_const_flow();
    let mut b = FnBuilder::new(&flow.core, "$emb_const_origins$v1$0");
    b.set_constant();
    b.term(Term::Return(None));
    flow.add(b.finish());
    let errors = compile_flow(&flow, &Options::default()).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.kind == ErrorKind::InvalidFlowIr
                && error.message.contains("const origin marker"))
    );
}

#[test]
fn distinct_direct_flow_const_ids_keep_independent_caches_without_source_markers() {
    let mut flow = external_const_flow();
    let mut second = flow.functions[emela_lir::FnId::from_raw(1.into())].clone();
    second.name = "OTHER".into();
    flow.add(second);
    let output = compile_flow(&flow, &Options::default()).unwrap();
    compile_and_run(
        output,
        &format!(
            r#"{BOARD}
static int calls;
int32_t fixture_seed(void) {{ return ++calls + 41; }}
int main(void) {{ for (int i=0;i<1000;i++) if(emela_export_VALUE_1()!=42 || emela_export_OTHER_3()!=43) return 1; return calls==2 ? 0 : 2; }}
"#
        ),
    );
}

#[test]
fn generic_function_references_returned_by_consts_keep_their_source_family() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
fn identity[A](value: A, unit: ()) -> A { value }
const IDENTITY = (identity, ())
pub fn integer() -> Int { (identity, _) = IDENTITY
 identity(42, ()) }
pub fn text() -> String { (identity, _) = IDENTITY
 identity("42", ()) }
"#,
        )],
        Roots::Public,
    )
    .unwrap();
    let flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    let (_, marker) = flow
        .functions
        .iter()
        .find(|(_, f)| f.name.starts_with("$emb_fn_origins$v1$"))
        .expect("ordinary FnRef specializations retain their source origin");
    assert_eq!(marker.stmts.len(), 3);
    for (_, stmt) in marker.stmts.iter() {
        let emela_lir::Stmt::Let {
            value: Value::FnRef(target),
            ..
        } = stmt
        else {
            panic!("family is a list of FnRef values")
        };
        assert!(flow.functions[*target].name.starts_with("identity["));
        assert!(!flow.functions[*target].constant);
    }
    compile_flow(&flow, &Options::default())
        .expect("well-shaped function family markers are unreachable metadata");
    let mut malformed = flow.clone();
    let (id, _) = malformed
        .functions
        .iter()
        .find(|(_, f)| f.name.starts_with("$emb_fn_origins$v1$"))
        .unwrap();
    malformed.functions[id].exported = true;
    let errors = compile_flow(&malformed, &Options::default()).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.kind == ErrorKind::InvalidFlowIr && e.message.contains("origin marker"))
    );
}

#[test]
fn source_polymorphic_consts_share_initialization_and_specialize_callable_families() {
    let source = r#"
@external(c, "tick")
fn tick() -> Int use {}
@external(c, "mark")
fn mark(n: Int) -> () use {}
const EMPTY = { dbg("EMPTY")
 []
}
const MISSING = { dbg("MISSING")
 None
}
const IDENTITY = { dbg("IDENTITY")
 (fn(value) { value }, ())
}
const APPLY = { dbg("APPLY")
 (fn(callback) { callback() }, ())
}
const CHOICE = { dbg("CHOICE")
 if tick() == 1 { (fn(value) { mark(1)
 value }, ()) } else { (fn(value) { mark(2)
 value }, ()) }
}
const CAPTURE = { dbg("CAPTURE")
 text = Int.to_string(tick())
 (fn(value) { fn() { (value, text) } }, ())
}
fn ints(xs: List[Int], o: Option[Int]) -> Int { List.length(xs) + (o match {Some(n)->n
 None->0}) }
fn texts(xs: List[String], o: Option[String]) -> Int { List.length(xs) + (o match {Some(s)->String.byte_size(s)
 None->0}) }
pub fn integer() -> Int { (capture,_) = CAPTURE
 (identity,_) = IDENTITY
 (apply,_) = APPLY
 (choice,_) = CHOICE
 (value,text)=capture(identity(20))()
 ints(EMPTY,MISSING) + value + apply(fn(){21}) + choice(1) + String.byte_size(text) - 1 }
pub fn text() -> Int { (capture,_) = CAPTURE
 (identity,_) = IDENTITY
 (apply,_) = APPLY
 (choice,_) = CHOICE
 (value,text)=capture(identity("42"))()
 texts(EMPTY,MISSING) + String.byte_size(value) + String.byte_size(apply(fn(){"42"})) + String.byte_size(choice("")) + String.byte_size(text) + 37 }
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Public).unwrap();
    let flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    for cycles in [false, true] {
        let mut flow = flow.clone();
        if cycles {
            gc_anchor::add(&mut flow);
        }
        let mut output = compile_flow(&flow, &Options::default().with_heap_bytes(8192)).unwrap();
        let integer = export(&output, "integer");
        let text = export(&output, "text");
        output.files.iter_mut().find(|f|f.path=="emela_embedded.c").unwrap().contents.push_str(r#"
unsigned test_live(void){unsigned n=0;if(!emela_heap_initialized)return 0;for(unsigned char*p=emela_heap_storage;p<emela_heap_end();){emela_heap_block*b=(emela_heap_block*)(void*)p;n+=!b->is_free;p+=b->size;}return n;}
"#);
        for choice in [1, 2] {
            compile_and_run(
                output.clone(),
                &format!(
                    r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
unsigned test_live(void);
static unsigned initializations,ticks,marks;
int32_t tick(void){{++ticks;return {choice};}}
void mark(int32_t n){{assert(n=={choice});++marks;}}
void emela_board_debug(const char*s,uint16_t n){{(void)s;(void)n;++initializations;}}
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*s,uint16_t n){{(void)k;(void)s;(void)n;abort();}}
int main(void){{assert({integer}()==42);assert({text}()==42);unsigned baseline=test_live();assert(initializations==6&&ticks==2);for(unsigned i=0;i<1000;i++){{assert({integer}()==42);assert({text}()==42);assert(test_live()==baseline);}}assert(initializations==6&&ticks==2&&marks==2002);return 0;}}
"#
                ),
            );
        }
    }
}

#[test]
fn polymorphic_const_callable_families_preserve_failure_evidence_and_selected_origins() {
    let core = support::try_lower(
        &[(
            "main.emel",
            include_str!("../../examples/support/const_polymorphic.emel"),
        )],
        Roots::Public,
    )
    .unwrap();
    let flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    for cycles in [false, true] {
        let mut flow = flow.clone();
        if cycles {
            gc_anchor::add(&mut flow);
        }
        let mut output = compile_flow(
            &flow,
            &Options::default()
                .with_heap_bytes(32768)
                .with_frame_bytes(1024),
        )
        .unwrap();
        let names = [
            "integer",
            "text",
            "evidence",
            "failing",
            "choice_integer",
            "choice_text",
            "enum_integer",
            "enum_text",
            "caught",
        ]
        .map(|n| export(&output, n));
        output.files.iter_mut().find(|f|f.path=="emela_embedded.c").unwrap().contents.push_str(r#"
unsigned test_live(void){unsigned n=0;if(!emela_heap_initialized)return 0;for(unsigned char*p=emela_heap_storage;p<emela_heap_end();){emela_heap_block*b=(emela_heap_block*)(void*)p;n+=!b->is_free;p+=b->size;}return n;}
"#);
        let [
            integer,
            text,
            evidence,
            failing,
            choice_integer,
            choice_text,
            enum_integer,
            enum_text,
            caught,
        ] = names;
        compile_and_run(
            output,
            &format!(
                r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
unsigned test_live(void);
static unsigned initializations;
void emela_board_debug(const char*s,uint16_t n){{(void)s;(void)n;++initializations;}}
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*s,uint16_t n){{fprintf(stderr,"%u: %.*s\n",k,n,s);abort();}}
int main(void){{unsigned baseline=0;for(unsigned i=0;i<1001;i++){{
 assert({integer}()==42);assert({text}()==42);assert({evidence}()==42);assert({failing}(0)==42);
 assert({choice_integer}()==42);assert({choice_text}()==42);assert({enum_integer}()==42);assert({enum_text}()==42);assert({caught}()==42);
 if(i==0)baseline=test_live();else assert(test_live()==baseline);
 }}assert(initializations==9);return 0;}}
"#
            ),
        );
    }
}

#[test]
fn polymorphic_const_opaque_handles_forward_one_pointer_abi_and_one_initialization() {
    let source = r#"
@external
type Handle[A]
@external(c,"host_handle")
fn handle[A]() -> Handle[A] use {}
const HANDLE = handle()
fn number(h: Handle[Int]) -> Int { 42 }
fn text(h: Handle[String]) -> Int { 42 }
pub fn work() -> Int { number(HANDLE) + text(HANDLE) - 42 }
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Public).unwrap();
    let options = Options::default().with_external_adapter(
        "host_handle",
        emela_codegen_emb::ExternalAdapter::new(
            "board_handle",
            emela_codegen_emb::ExternalResult::Borrowed,
        ),
    );
    let output = compile(&core, &options).unwrap();
    let work = export(&output, "work");
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
static unsigned calls;static int token;
void *board_handle(void){{++calls;return &token;}}
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*s,uint16_t n){{(void)k;(void)s;(void)n;abort();}}
int main(void){{for(unsigned i=0;i<1000;i++)assert({work}()==42);assert(calls==1);return 0;}}
"#
        ),
    );
}

#[test]
fn polymorphic_empty_list_const_keeps_the_heapless_representation() {
    let source = "const EMPTY=[]\nfn ints(xs:List[Int])->Int{if List.is_empty(xs) {0}else{1}}\nfn texts(xs:List[String])->Int{if List.is_empty(xs) {0}else{1}}\npub fn work()->Int{ints(EMPTY)+texts(EMPTY)+42}\n";
    let output = emit_source(source, 0);
    let work = export(&output, "work");
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
            "{BOARD}\nint main(void){{for(unsigned i=0;i<1000;i++)if({work}()!=42)return 1;return 0;}}\n"
        ),
    );
}

#[test]
fn canonical_const_roots_include_dependency_specializations_until_they_reach_a_fixed_point() {
    let core = support::try_lower(
        &[(
            "main.emel",
            include_str!("../../examples/support/const_polymorphic_dependency.emel"),
        )],
        Roots::Public,
    )
    .unwrap();
    let flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    let names: Vec<_> = flow
        .functions
        .iter()
        .filter(|(_, f)| f.name.starts_with("$emb_const_origins$v1$"))
        .map(|(_, f)| {
            let seq = &f.seqs[f.body.unwrap()];
            let emela_lir::Stmt::Let {
                value: Value::FnRef(canonical),
                ..
            } = f.stmts[seq.stmts[0]]
            else {
                panic!("canonical origin")
            };
            flow.functions[canonical].name.as_str()
        })
        .collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"BASE"));
    assert!(names.contains(&"PAIR"));
}

#[test]
fn polymorphic_const_defects_have_one_source_context_and_self_reference_retries() {
    for recursive in [false, true] {
        let expression = if recursive {
            "BROKEN"
        } else {
            "panic(\"broken\")"
        };
        let source = format!(
            r#"
@external(c,"trigger")
fn trigger() -> Int use {{}}
const BROKEN = {{ dbg("once")
 if trigger() == 1 {{ {expression} }} else {{ [] }}
}}
fn ints(xs:List[Int])->Int{{if List.is_empty(xs){{42}}else{{0}}}}
fn texts(xs:List[String])->Int{{if List.is_empty(xs){{42}}else{{0}}}}
pub fn integer()->Int{{ints(BROKEN)}}
pub fn text()->Int{{texts(BROKEN)}}
"#
        );
        let mut output = emit_source(&source, 1024);
        let integer = export(&output, "integer");
        let text = export(&output, "text");
        output.files.iter_mut().find(|f|f.path=="emela_embedded.c").unwrap().contents.push_str("int test_heap_empty(void){if(!emela_heap_initialized)return 1;for(unsigned char*p=emela_heap_storage;p<emela_heap_end();){emela_heap_block*b=(emela_heap_block*)(void*)p;if(!b->is_free)return 0;p+=b->size;}return 1;}\n");
        let expected = if recursive {
            "const BROKEN refers to itself while being evaluated"
        } else {
            "while evaluating const BROKEN: broken"
        };
        let count = if recursive { 2000 } else { 1 };
        compile_and_run(
            output,
            &format!(
                r#"
#include <assert.h>
#include <stdlib.h>
#include <setjmp.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);static jmp_buf escape;static unsigned calls,debugs;
int32_t trigger(void){{++calls;return 1;}}
void emela_board_debug(const char*s,uint16_t n){{(void)s;(void)n;++debugs;}}
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*s,uint16_t n){{(void)k;assert(n==sizeof("{expected}")-1);assert(!memcmp(s,"{expected}",n));longjmp(escape,1);}}
int main(void){{for(unsigned i=0;i<1000;i++){{if(setjmp(escape)==0){{{integer}();abort();}}if(setjmp(escape)==0){{{text}();abort();}}assert(test_heap_empty());}}assert(calls=={count}&&debugs=={count});return 0;}}
"#
            ),
        );
    }
}

#[test]
fn polymorphic_const_bridge_capacity_defects_use_the_source_name_once_and_are_cached() {
    let source = r#"
const ITEMS={dbg("once")
 [[]]
}
fn ints(xs:List[List[Int]])->Int{42}
fn texts(xs:List[List[String]])->Int{42}
pub fn integer()->Int{ints(ITEMS)}
pub fn text()->Int{texts(ITEMS)}
"#;
    let output = emit_source(source, 96);
    let integer = export(&output, "integer");
    let text = export(&output, "text");
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include <setjmp.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static jmp_buf escape;static unsigned debugs;
void emela_board_debug(const char*s,uint16_t n){{(void)s;(void)n;++debugs;}}
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*s,uint16_t n){{const char expected[]="while evaluating const ITEMS: fixed heap exhausted";assert(k==EMELA_DEFECT_HEAP_EXHAUSTED);assert(n==sizeof expected-1&&!memcmp(s,expected,n));longjmp(escape,1);}}
int main(void){{for(unsigned i=0;i<1000;i++){{if(setjmp(escape)==0){{{integer}();abort();}}if(setjmp(escape)==0){{{text}();abort();}}}}assert(debugs==1);return 0;}}
"#
        ),
    );
}

#[test]
fn canonical_dependency_consts_initialize_once_across_nested_specializations() {
    let output = emit_source(
        include_str!("../../examples/support/const_polymorphic_dependency.emel"),
        8192,
    );
    let integer = export(&output, "integer");
    let text = export(&output, "text");
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
static unsigned initializations;
void emela_board_debug(const char*s,uint16_t n){{(void)s;(void)n;++initializations;}}
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*s,uint16_t n){{(void)k;(void)s;(void)n;abort();}}
int main(void){{for(unsigned i=0;i<1000;i++){{assert({integer}()==42);assert({text}()==42);}}assert(initializations==2);return 0;}}
"#
        ),
    );
}

#[test]
fn oversized_static_string_growth_falls_back_before_unbounded_compiler_allocation() {
    let mut expression = "\"x\"".to_owned();
    for _ in 0..17 {
        expression = format!("double({expression})");
    }
    let source = format!(
        "fn double(value: String) -> String {{ String.append(value,value) }}\nconst VALUE = {expression}\npub fn size() -> Int {{ String.byte_size(VALUE) }}"
    );
    let core = support::try_lower(&[("main.emel", &source)], Roots::Public).unwrap();
    let output = compile(
        &core,
        &Options::default()
            .with_heap_bytes(1024)
            .with_string_capacity(32),
    )
    .expect("large static results use bounded runtime const evaluation");
    let size = export(&output, "size");
    compile_and_run(
        output,
        &format!(
            r#"
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind kind,const char *message,uint16_t length) {{
 static const char prefix[]="while evaluating const VALUE: ";
 exit(kind==EMELA_DEFECT_STRING_CAPACITY && length>=sizeof prefix-1 && !memcmp(message,prefix,sizeof prefix-1)?0:2);
}}
int main(void) {{(void){size}();return 1;}}
"#
        ),
    );
}
