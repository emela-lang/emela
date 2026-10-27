use super::*;

#[derive(Clone, Copy, Debug)]
enum Alias {
    None,
    Direct,
    Evidence,
    Coerce,
    Tuple,
    Argument,
}

fn flow(alias: Alias, suspends: bool) -> Module {
    let mut flow = Module::new(emela_core::Module::new());
    let effect = flow.core.add_effect(emela_core::EffectDef::new(
        "Read",
        vec![emela_core::OpDef::new("get", vec![], TypeId::INT)],
    ));
    let handler_ty = flow.core.types.intern(Type::Handler(effect));
    let tuple_ty = flow.core.types.intern(Type::Tuple(vec![handler_ty]));
    let operation_ty = flow
        .core
        .types
        .intern(Type::Fn(FnType::pure(vec![], TypeId::INT)));
    let check = flow.declare("check");
    let retire = if matches!(alias, Alias::Argument) {
        let mut b = FnBuilder::new(&flow.core, "retire");
        let h = b.param("handler", handler_ty);
        b.let_(
            None,
            Value::Runtime {
                op: RuntimeOp::Retire,
                args: vec![h.into(), Const::String("Audit".into()).into()],
            },
        );
        b.term(Term::Return(None));
        Some(flow.add(b.finish()))
    } else {
        None
    };
    let mut b = FnBuilder::new(&flow.core, "check");
    b.set_exported().set_ret(TypeId::INT);
    if suspends {
        b.set_suspends();
    }
    let op = b.var("operation", operation_ty);
    b.closure(op, vec![], vec![], false, |b| {
        b.term(Term::Return(Some(Const::Int(7).into())));
    });
    let h = b.assign(
        "handler",
        handler_ty,
        Value::Handler {
            name: "Audit".into(),
            effect,
            ops: vec![op],
        },
    );
    let retired = match alias {
        Alias::None | Alias::Direct | Alias::Argument => h,
        Alias::Coerce => b.assign("alias", handler_ty, Value::Coerce(h)),
        Alias::Tuple => {
            let tuple = b.assign("tuple", tuple_ty, Value::Tuple(vec![h.into()]));
            b.assign(
                "alias",
                handler_ty,
                Value::TupleGet {
                    base: tuple,
                    index: 0,
                },
            )
        }
        Alias::Evidence => {
            let ev = b.assign(
                "evidence",
                TypeId::EVIDENCE,
                Value::EvExtend {
                    base: None,
                    bindings: vec![(effect, h)],
                },
            );
            b.assign("alias", handler_ty, Value::EvLookup { ev, effect })
        }
    };
    if let Some(retire) = retire {
        b.let_(
            None,
            Value::Call {
                callee: Callee::Direct(retire),
                args: vec![h.into()],
                suspends: false,
                on_fail: None,
            },
        );
    } else if !matches!(alias, Alias::None) {
        b.let_(
            None,
            Value::Runtime {
                op: RuntimeOp::Retire,
                args: vec![retired.into(), Const::String("Audit".into()).into()],
            },
        );
    }
    let result = b.assign(
        "result",
        TypeId::INT,
        Value::Call {
            callee: Callee::Op {
                handler: h,
                effect,
                op: 0,
            },
            args: vec![],
            suspends: false,
            on_fail: None,
        },
    );
    b.term(Term::Return(Some(result.into())));
    flow.define(check, b.finish());
    flow
}

#[test]
fn retired_handler_aliases_keep_the_active_check_in_native_and_suspend_callers() {
    let aliases = [
        Alias::Direct,
        Alias::Evidence,
        Alias::Coerce,
        Alias::Tuple,
        Alias::Argument,
    ];
    for_each_in_parallel(aliases, |alias| {
        for suspends in [false, true] {
            let flow = flow(alias, suspends);
            emela_lir::verify(&flow).expect("retiring an alias is verified Flow");
            let output = compile_flow(&flow, &Options::default().with_heap_bytes(2048))
                .unwrap_or_else(|e| panic!("{alias:?}, suspend={suspends}: {e:?}"));
            let invoke = if suspends {
                "int32_t result; (void)emela_export_check_0_start(&result); while (emela_scheduler_run(1)) {}"
            } else {
                "(void)emela_export_check_0();"
            };
            compile_and_run(
                output,
                &format!(
                    r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {{
  static const char expected[] = "handler Audit was used after release";
  assert(kind == EMELA_DEFECT_PANIC && length == sizeof(expected) - 1);
  assert(memcmp(message, expected, length) == 0); exit(0);
}}
int main(void) {{ {invoke} return 1; }}
"#
                ),
            );
        }
    });
}

#[test]
fn unretired_static_handlers_keep_the_heapless_dispatch_path() {
    let output = compile_flow(&flow(Alias::None, false), &Options::default()).unwrap();
    assert!(
        !output
            .files
            .iter()
            .any(|file| file.contents.contains("emela_heap_storage"))
    );
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) { (void)k;(void)m;(void)n;abort(); }
int main(void) { assert(emela_export_check_0() == 7); }
"#,
    );
}
