use super::*;

mod discarded;

fn discarded_value_flow(kind: &str) -> Module {
    let mut core = emela_core::Module::new();
    let boxed = core.add_enum(EnumDef {
        name: "Boxed".into(),
        params: 0,
        variants: vec![VariantDef::positional(
            "Boxed",
            vec![TypeId::INT, TypeId::UNIT],
        )],
        error: false,
        external: false,
    });
    let boxed_ty = core.enum_type(boxed, vec![]);
    let tuple_ty = core.tuple(vec![TypeId::INT, TypeId::UNIT]);
    let ints = core.list(TypeId::INT);
    let units = core.list(TypeId::UNIT);
    let error_id = core.add_enum(EnumDef {
        name: "Ignored".into(),
        params: 0,
        variants: vec![VariantDef::positional("Ignored", vec![TypeId::INT])],
        error: true,
        external: false,
    });
    let error_ty = core.enum_type(error_id, vec![]);
    let mut flow = Module::new(core);
    let mut target = FnBuilder::new(&flow.core, "target");
    target.set_ret(TypeId::INT);
    if kind == "is_error" || kind == "as_error" {
        target.set_fails();
        let error = target.assign(
            "raised",
            error_ty,
            Value::Make {
                ctor: CtorRef {
                    enum_id: error_id,
                    variant: 0,
                },
                args: vec![Const::Int(42).into()],
            },
        );
        target.term(Term::Fail(error.into()));
    } else {
        target.term(Term::Return(Some(Const::Int(42).into())));
    }
    let target = flow.add(target.finish());
    let mut b = FnBuilder::new(&flow.core, "run");
    b.set_exported().set_ret(TypeId::INT);
    match kind {
        "field" => {
            let base = b.assign(
                "base",
                boxed_ty,
                Value::Make {
                    ctor: CtorRef {
                        enum_id: boxed,
                        variant: 0,
                    },
                    args: vec![Const::Int(42).into(), Const::Unit.into()],
                },
            );
            for index in 0..2 {
                b.let_(
                    None,
                    Value::Field {
                        base,
                        ctor: CtorRef {
                            enum_id: boxed,
                            variant: 0,
                        },
                        index,
                    },
                );
            }
        }
        "tuple_get" => {
            let base = b.assign(
                "base",
                tuple_ty,
                Value::Tuple(vec![Const::Int(42).into(), Const::Unit.into()]),
            );
            for index in 0..2 {
                b.let_(None, Value::TupleGet { base, index });
            }
        }
        "list_empty" => {
            let list = b.assign(
                "list",
                ints,
                Value::List {
                    elems: vec![],
                    tail: None,
                },
            );
            b.let_(None, Value::ListIsEmpty(list));
        }
        "list_head" => {
            let list = b.assign(
                "list",
                ints,
                Value::List {
                    elems: vec![Const::Int(42).into()],
                    tail: None,
                },
            );
            b.let_(None, Value::ListHead(list));
            let unit = b.assign(
                "unit",
                units,
                Value::List {
                    elems: vec![Const::Unit.into()],
                    tail: None,
                },
            );
            b.let_(None, Value::ListHead(unit));
        }
        "list_tail" => {
            let list = b.assign(
                "list",
                ints,
                Value::List {
                    elems: vec![Const::Int(42).into()],
                    tail: None,
                },
            );
            b.let_(None, Value::ListTail(list));
        }
        "fn_ref" => b.let_(None, Value::FnRef(target)),
        "is_error" | "as_error" => {
            let error = b.var("caught", TypeId::ANY);
            b.block(Some(error), "catch", |b, label| {
                b.let_(
                    None,
                    Value::Call {
                        callee: Callee::Direct(target),
                        args: vec![],
                        suspends: false,
                        on_fail: Some(OnFail::Break(label)),
                    },
                );
                b.term(Term::Unreachable);
            });
            b.let_(
                None,
                if kind == "is_error" {
                    Value::IsError {
                        error,
                        enum_id: error_id,
                    }
                } else {
                    Value::AsError {
                        error,
                        enum_id: error_id,
                    }
                },
            );
        }
        "coerce" => {
            let value = b.assign("value", TypeId::INT, Value::Const(Const::Int(42)));
            b.let_(None, Value::Coerce(value));
        }
        _ => panic!("unknown fixture"),
    }
    b.term(Term::Return(Some(Const::Int(42).into())));
    flow.add(b.finish());
    flow
}

fn run_discarded(kind: &str, heap: u32) {
    let flow = discarded_value_flow(kind);
    emela_lir::verify(&flow).expect("the discarded value is valid Flow IR");
    let mut output = compile_flow(&flow, &Options::default().with_heap_bytes(heap))
        .expect("emit the discarded value");
    let mut extra = String::new();
    if heap != 0 {
        output
            .files
            .iter_mut()
            .find(|f| f.path == "emela_embedded.c")
            .unwrap()
            .contents
            .push_str(
                r#"
int test_heap_empty(void) {
  if(!emela_heap_initialized) return 1;
  for(unsigned char *p=emela_heap_storage;p<emela_heap_end();) {
    emela_heap_block *block=(emela_heap_block *)(void *)p;
    if(!block->is_free) return 0;
    p+=block->size;
  }
  return 1;
}
"#,
            );
        extra.push_str("assert(test_heap_empty());");
    }
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *s,uint16_t n) {{ (void)k; (void)s; (void)n; abort(); }}
int main(void) {{ for(unsigned i=0;i<1000;i++) {{ assert(emela_export_run_1()==42); {extra} }} return 0; }}
"#
        ),
    );
}

#[test]
fn discarded_enum_fields_accept_represented_and_unit_fields() {
    run_discarded("field", 0);
}
#[test]
fn discarded_tuple_projections_accept_represented_and_unit_fields() {
    run_discarded("tuple_get", 0);
}
#[test]
fn discarded_list_emptiness_is_heapless() {
    run_discarded("list_empty", 0);
}
#[test]
fn discarded_list_heads_accept_represented_and_unit_elements() {
    run_discarded("list_head", 2048);
}
#[test]
fn discarded_list_tail_preserves_the_source_owner() {
    run_discarded("list_tail", 2048);
}
#[test]
fn discarded_top_level_function_reference_is_heapless() {
    run_discarded("fn_ref", 0);
}
#[test]
fn discarded_error_test_accepts_a_caught_error_descriptor() {
    run_discarded("is_error", 0);
}
#[test]
fn discarded_error_projection_accepts_a_caught_error_descriptor() {
    run_discarded("as_error", 0);
}
#[test]
fn discarded_coerce_without_a_target_type_is_a_noop() {
    run_discarded("coerce", 0);
}

#[test]
fn discarded_integer_division_and_remainder_still_raise_defects_in_sync_and_suspend_code() {
    for suspends in [false, true] {
        for ty in [PrimTy::I32, PrimTy::I64] {
            for op in [PrimOp::Div, PrimOp::Rem] {
                let core = emela_core::Module::new();
                let mut b = FnBuilder::new(&core, "probe");
                b.set_exported();
                if suspends {
                    b.set_suspends();
                }
                let args = if ty == PrimTy::I32 {
                    vec![Const::Int(7).into(), Const::Int(0).into()]
                } else {
                    vec![Const::Int64(7).into(), Const::Int64(0).into()]
                };
                b.let_(None, Value::Prim { op, ty, args });
                b.term(Term::Return(None));
                let function = b.finish();
                let mut flow = Module::new(core);
                flow.add(function);
                emela_lir::verify(&flow).unwrap();
                let output = compile_flow(&flow, &Options::default()).unwrap();
                let call = if suspends {
                    "(void)emela_export_probe_0_start(); while(emela_scheduler_run(1)) {}"
                } else {
                    "emela_export_probe_0();"
                };
                compile_and_run(
                    output,
                    &format!(
                        r#"
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
uint32_t emela_board_event_lock(void) {{ return 0; }}
void emela_board_event_unlock(uint32_t n) {{ (void)n; }}
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *s,uint16_t n) {{ (void)s; (void)n; exit(k==EMELA_DEFECT_DIVISION_BY_ZERO?0:2); }}
int main(void) {{ {call} return 1; }}
"#
                    ),
                );
            }
        }
    }
}

#[test]
fn changing_recursive_type_arguments_with_a_finite_type_graph_are_supported() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
pub enum Nest[A] { End(A), Next(Nest[Bool]) }
pub fn make() -> Nest[Int] { End(42) }
"#,
        )],
        Roots::Public,
    )
    .expect("finite recursive type arguments are legal source types");
    let flow = emela_codegen_emb::flow_from_core(&core)
        .expect("lower the finite type graph to verified Flow IR");
    emela_lir::verify(&flow).unwrap();
    compile_flow(&flow, &Options::default().with_heap_bytes(2048))
        .expect("finite recursive instantiations have a concrete C layout");
}

#[test]
fn any_unboxing_preserves_its_runtime_type_check() {
    let core = emela_core::Module::new();
    let mut b = FnBuilder::new(&core, "run");
    b.set_exported().set_ret(TypeId::STRING);
    let number = b.assign("number", TypeId::INT, Value::Const(Const::Int(42)));
    let boxed = b.assign("boxed", TypeId::ANY, Value::Coerce(number));
    let text = b.assign("text", TypeId::STRING, Value::Coerce(boxed));
    b.term(Term::Return(Some(text.into())));
    let function = b.finish();
    let mut flow = Module::new(core);
    flow.add(function);
    emela_lir::verify(&flow).unwrap();
    let output = compile_flow(&flow, &Options::default().with_heap_bytes(2048)).unwrap();
    compile_and_run(
        output,
        r#"
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *s,uint16_t n) { (void)s; (void)n; exit(k==EMELA_DEFECT_PANIC && n==17?0:2); }
int main(void) { (void)emela_export_run_0(); return 1; }
"#,
    );
}
