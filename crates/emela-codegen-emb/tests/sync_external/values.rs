use super::*;

#[test]
fn source_tuple_and_list_external_values_keep_all_managed_fields_alive() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
@external(c,"host_packet")
fn packet(p: (String,List[String])) -> (String,List[String]) use {}
pub fn roundtrip(s: String) -> (String,List[String]) { packet((s,[s])) }
pub fn text(p: (String,List[String])) -> String { (s,_) = p
 s }
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    let mut output = compile(
        &core,
        &Options::default()
            .with_heap_bytes(1024)
            .with_external_adapter(
                "host_packet",
                ExternalAdapter::new("board_packet", ExternalResult::Borrowed),
            ),
    )
    .unwrap();
    add_heap_check(&mut output);
    let header = &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.h")
        .unwrap()
        .contents;
    let cty = header
        .lines()
        .find(|line| line.contains(" board_packet("))
        .unwrap()
        .split(" board_packet(")
        .next()
        .unwrap();
    let release = header
        .lines()
        .find(|line| {
            line.starts_with("void emela_external_release_")
                && line.contains(&format!("({cty} *value)"))
        })
        .unwrap()
        .strip_prefix("void ")
        .unwrap()
        .split('(')
        .next()
        .unwrap();
    compile_and_run(
        output.clone(),
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) {{(void)k;(void)m;(void)n;abort();}}
{cty} board_packet({cty} packet) {{return packet;}}
int main(void) {{for(int i=0;i<1000;i++) {{char input[]={{'a','\0','b'}};{cty} packet=emela_export_roundtrip_0((EmelaString){{.data=input,.length=3}});memset(input,'x',3);
 EmelaString text=emela_export_text_1(packet);{release}(&packet);assert(text.owner&&text.length==3&&!memcmp(text.data,"a\0b",3));emela_embedded_release_string(&text);assert(test_heap_empty());}}}}
"#
        ),
    );
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Shape {
    String,
    Tuple,
    Enum,
    List,
    Any,
    ErrorAny,
    AnyTuple,
    Recursive,
    Cell,
    Function,
    Handler,
    Evidence,
}

pub(super) fn fixture(shape: Shape) -> (Module, TypeId, u32, u32) {
    let mut core = emela_core::Module::new();
    let list = core.types.intern(Type::List(TypeId::STRING));
    let tuple = core.types.intern(Type::Tuple(vec![TypeId::STRING, list]));
    let function = core
        .types
        .intern(Type::Fn(FnType::pure(vec![], TypeId::STRING)));
    let packet = core.add_enum(EnumDef {
        name: "Packet".into(),
        params: 0,
        variants: vec![VariantDef::positional("Packet", vec![TypeId::STRING, list])],
        error: false,
        external: false,
    });
    let packet_ty = core.enum_type(packet, vec![]);
    let error = core.add_enum(EnumDef {
        name: "Boom".into(),
        params: 0,
        variants: vec![VariantDef::positional("Boom", vec![TypeId::STRING, list])],
        error: true,
        external: false,
    });
    let error_ty = core.enum_type(error, vec![]);
    let chain = core.add_enum(EnumDef {
        name: "Chain".into(),
        params: 0,
        variants: vec![
            VariantDef::unit("End"),
            VariantDef::positional("Node", vec![TypeId::STRING, TypeId::UNIT]),
        ],
        error: false,
        external: false,
    });
    let chain_ty = core.enum_type(chain, vec![]);
    core.enums[chain].variants[1].tys[1] = chain_ty;
    let effect = core.add_effect(emela_core::EffectDef::new(
        "Read",
        vec![emela_core::OpDef::new("get", vec![], TypeId::STRING)],
    ));
    let handler_ty = core.types.intern(Type::Handler(effect));
    let cell = core.types.intern(Type::Cell(TypeId::STRING));
    let any_tuple = core
        .types
        .intern(Type::Tuple(vec![TypeId::ANY, TypeId::STRING]));
    let ty = match shape {
        Shape::String => TypeId::STRING,
        Shape::Tuple => tuple,
        Shape::Enum => packet_ty,
        Shape::List => list,
        Shape::Any | Shape::ErrorAny => TypeId::ANY,
        Shape::AnyTuple => any_tuple,
        Shape::Recursive => chain_ty,
        Shape::Cell => cell,
        Shape::Function => function,
        Shape::Handler => handler_ty,
        Shape::Evidence => TypeId::EVIDENCE,
    };
    let callback = core.types.intern(Type::Fn(FnType::pure(vec![ty], ty)));
    let suspended_callback = core.types.intern(Type::Fn(FnType {
        suspends: true,
        ..FnType::pure(vec![ty], ty)
    }));
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "external");
    b.set_exported().set_ret(ty);
    b.param("value", ty);
    let external = flow.add(b.finish_external(binding("host_roundtrip")));
    for (mode, name) in ["direct", "indirect", "coerced", "suspended", "discard"]
        .into_iter()
        .enumerate()
    {
        let mut b = FnBuilder::new(&flow.core, name);
        b.set_exported()
            .set_ret(if mode == 4 { TypeId::UNIT } else { ty });
        if mode == 2 || mode == 3 {
            b.set_suspends();
        }
        let input = b.param("value", ty);
        let callee = if mode == 1 || mode == 2 {
            let f = b.assign("callback", callback, Value::FnRef(external));
            Callee::Indirect(if mode == 2 {
                b.assign("coerced", suspended_callback, Value::Coerce(f))
            } else {
                f
            })
        } else {
            Callee::Direct(external)
        };
        let value = Value::Call {
            callee,
            args: vec![input.into()],
            suspends: mode == 2,
            on_fail: None,
        };
        if mode == 4 {
            b.let_(None, value);
            b.term(Term::Return(None));
        } else {
            let result = b.assign("result", ty, value);
            b.term(Term::Return(Some(result.into())));
        }
        flow.add(b.finish());
    }
    let raise = if matches!(shape, Shape::ErrorAny | Shape::AnyTuple) {
        let mut b = FnBuilder::new(&flow.core, "raise");
        b.set_fails();
        let error = b.param("error", error_ty);
        b.term(Term::Fail(error.into()));
        Some(flow.add(b.finish()))
    } else {
        None
    };
    let mut b = FnBuilder::new(&flow.core, "make");
    b.set_exported().set_ret(ty);
    let text = b.param("text", TypeId::STRING);
    let value = match shape {
        Shape::String => text,
        Shape::Cell => b.assign(
            "cell",
            ty,
            Value::Builtin {
                op: Builtin::CellNew,
                args: vec![text.into()],
            },
        ),
        Shape::Function | Shape::Handler | Shape::Evidence => {
            let f = b.var("capture", function);
            b.closure(f, vec![], vec![text], false, |b| {
                b.term(Term::Return(Some(text.into())))
            });
            if matches!(shape, Shape::Function) {
                f
            } else {
                let h = b.assign(
                    "handler",
                    handler_ty,
                    Value::Handler {
                        name: "Board".into(),
                        effect,
                        ops: vec![f],
                    },
                );
                if matches!(shape, Shape::Handler) {
                    h
                } else {
                    b.assign(
                        "evidence",
                        TypeId::EVIDENCE,
                        Value::EvExtend {
                            base: None,
                            bindings: vec![(effect, h)],
                        },
                    )
                }
            }
        }
        Shape::Recursive => {
            let end = b.assign(
                "end",
                ty,
                Value::Make {
                    ctor: CtorRef {
                        enum_id: chain,
                        variant: 0,
                    },
                    args: vec![],
                },
            );
            b.assign(
                "node",
                ty,
                Value::Make {
                    ctor: CtorRef {
                        enum_id: chain,
                        variant: 1,
                    },
                    args: vec![text.into(), end.into()],
                },
            )
        }
        _ => {
            let parts = b.assign(
                "parts",
                list,
                Value::List {
                    elems: vec![text.into()],
                    tail: None,
                },
            );
            match shape {
                Shape::List => parts,
                Shape::Enum => b.assign(
                    "packet",
                    ty,
                    Value::Make {
                        ctor: CtorRef {
                            enum_id: packet,
                            variant: 0,
                        },
                        args: vec![text.into(), parts.into()],
                    },
                ),
                Shape::Tuple => {
                    b.assign("tuple", ty, Value::Tuple(vec![text.into(), parts.into()]))
                }
                Shape::Any => {
                    let tuple = b.assign(
                        "tuple",
                        tuple,
                        Value::Tuple(vec![text.into(), parts.into()]),
                    );
                    b.assign("any", ty, Value::Coerce(tuple))
                }
                Shape::ErrorAny | Shape::AnyTuple => {
                    let error = b.assign(
                        "error",
                        error_ty,
                        Value::Make {
                            ctor: CtorRef {
                                enum_id: error,
                                variant: 0,
                            },
                            args: vec![text.into(), parts.into()],
                        },
                    );
                    let any = b.var("any", TypeId::ANY);
                    b.block(Some(any), "catch", |b, label| {
                        b.let_(
                            None,
                            Value::Call {
                                callee: Callee::Direct(raise.unwrap()),
                                args: vec![error.into()],
                                suspends: false,
                                on_fail: Some(OnFail::Break(label)),
                            },
                        );
                        b.term(Term::Defect(
                            Const::String("expected failure".into()).into(),
                            None,
                        ));
                    });
                    if matches!(shape, Shape::AnyTuple) {
                        b.assign("tuple", ty, Value::Tuple(vec![any.into(), text.into()]))
                    } else {
                        any
                    }
                }
                _ => unreachable!(),
            }
        }
    };
    b.term(Term::Return(Some(value.into())));
    let make = u32::from(flow.add(b.finish()).into_raw());
    let mut b = FnBuilder::new(&flow.core, "read");
    b.set_exported().set_ret(TypeId::STRING);
    let value = b.param("value", ty);
    let text = match shape {
        Shape::String => value,
        Shape::Tuple => b.assign(
            "text",
            TypeId::STRING,
            Value::TupleGet {
                base: value,
                index: 0,
            },
        ),
        Shape::Enum => b.assign(
            "text",
            TypeId::STRING,
            Value::Field {
                base: value,
                ctor: CtorRef {
                    enum_id: packet,
                    variant: 0,
                },
                index: 0,
            },
        ),
        Shape::List => b.assign("text", TypeId::STRING, Value::ListHead(value)),
        Shape::Recursive => b.assign(
            "text",
            TypeId::STRING,
            Value::Field {
                base: value,
                ctor: CtorRef {
                    enum_id: chain,
                    variant: 1,
                },
                index: 0,
            },
        ),
        Shape::Cell => b.assign(
            "text",
            TypeId::STRING,
            Value::Builtin {
                op: Builtin::CellGet,
                args: vec![value.into()],
            },
        ),
        Shape::Function => b.assign(
            "text",
            TypeId::STRING,
            Value::Call {
                callee: Callee::Indirect(value),
                args: vec![],
                suspends: false,
                on_fail: None,
            },
        ),
        Shape::Handler | Shape::Evidence => {
            let h = if matches!(shape, Shape::Evidence) {
                b.assign("handler", handler_ty, Value::EvLookup { ev: value, effect })
            } else {
                value
            };
            b.assign(
                "text",
                TypeId::STRING,
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
            )
        }
        Shape::Any => {
            let unboxed = b.assign("tuple", tuple, Value::Coerce(value));
            b.assign(
                "text",
                TypeId::STRING,
                Value::TupleGet {
                    base: unboxed,
                    index: 0,
                },
            )
        }
        Shape::ErrorAny | Shape::AnyTuple => {
            let value = if matches!(shape, Shape::AnyTuple) {
                b.assign(
                    "any",
                    TypeId::ANY,
                    Value::TupleGet {
                        base: value,
                        index: 0,
                    },
                )
            } else {
                value
            };
            let concrete = b.assign(
                "error",
                error_ty,
                Value::AsError {
                    error: value,
                    enum_id: error,
                },
            );
            b.assign(
                "text",
                TypeId::STRING,
                Value::Field {
                    base: concrete,
                    ctor: CtorRef {
                        enum_id: error,
                        variant: 0,
                    },
                    index: 0,
                },
            )
        }
    };
    b.term(Term::Return(Some(text.into())));
    let read = u32::from(flow.add(b.finish()).into_raw());
    (flow, ty, make, read)
}

#[test]
fn generated_value_adapters_roundtrip_managed_types_with_borrowed_and_owned_results() {
    for shape in [
        Shape::String,
        Shape::Tuple,
        Shape::Enum,
        Shape::List,
        Shape::Any,
        Shape::ErrorAny,
        Shape::AnyTuple,
        Shape::Recursive,
        Shape::Cell,
        Shape::Function,
        Shape::Handler,
        Shape::Evidence,
    ] {
        for result in [ExternalResult::Borrowed, ExternalResult::Owned] {
            let (flow, ty, make, read) = fixture(shape);
            let mut output = compile_flow(
                &flow,
                &Options::default()
                    .with_heap_bytes(2048)
                    .with_frame_bytes(1024)
                    .with_external_adapter(
                        "host_roundtrip",
                        ExternalAdapter::new("board_roundtrip", result),
                    ),
            )
            .unwrap_or_else(|e| panic!("{shape:?}/{result:?}: {e:?}"));
            add_heap_check(&mut output);
            let header = &output
                .files
                .iter()
                .find(|f| f.path == "emela_embedded.h")
                .unwrap()
                .contents;
            let cty = header
                .lines()
                .find(|s| s.contains(" board_roundtrip("))
                .unwrap()
                .split(" board_roundtrip(")
                .next()
                .unwrap();
            let own = if result == ExternalResult::Owned {
                format!("emela_external_own_{}(&value);", ty.index())
            } else {
                String::new()
            };
            let id = ty.index();
            compile_and_run(
                output.clone(),
                &format!(
                    r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) {{ (void)k;(void)m;(void)n;abort(); }}
{cty} board_roundtrip({cty} value) {{ {own} return value; }}
static void check({cty} value) {{ EmelaString text=emela_export_read_{read}(value);emela_external_release_{id}(&value);assert(text.length==3&&!memcmp(text.data,"a\0b",3));emela_embedded_release_string(&text); }}
int main(void) {{for(int i=0;i<1000;i++) {{
 char bytes[]={{'a','\0','b'}};{cty} input=emela_export_make_{make}((EmelaString){{.data=bytes,.length=3}});
 {cty} value=emela_export_external_0(input);memset(bytes,'x',3);check(value);
 bytes[0]='a';bytes[1]=0;bytes[2]='b';check(emela_export_direct_1(input));check(emela_export_indirect_2(input));
 value=({cty}){{0}};EmelaFiber f=emela_export_coerced_3_start(input,&value);while(!emela_fiber_done(f)) {{assert(emela_scheduler_run(1));}}emela_fiber_release(f);check(value);
 value=({cty}){{0}};f=emela_export_suspended_4_start(input,&value);while(!emela_fiber_done(f)) {{assert(emela_scheduler_run(1));}}emela_fiber_release(f);check(value);
 f=emela_export_suspended_4_start(input,NULL);while(!emela_fiber_done(f)) {{assert(emela_scheduler_run(1));}}emela_fiber_release(f);
 emela_export_discard_5(input);emela_external_release_{id}(&input);assert(test_heap_empty());
}}}}
"#
                ),
            );
        }
    }
}
