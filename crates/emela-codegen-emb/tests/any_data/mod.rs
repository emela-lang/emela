use super::*;

fn external(core: &mut emela_core::Module, name: &str) -> emela_core::EnumId {
    core.add_enum(EnumDef {
        name: name.into(),
        params: 1,
        variants: vec![],
        error: false,
        external: true,
    })
}

fn fixture(gc: bool) -> Module {
    let mut core = emela_core::Module::new();
    let list = core.list(TypeId::STRING);
    let error_id = core.add_enum(EnumDef {
        name: "DataError".into(),
        params: 0,
        variants: vec![VariantDef::positional(
            "DataError",
            vec![TypeId::STRING, list],
        )],
        error: true,
        external: false,
    });
    let error = core.enum_type(error_id, vec![]);
    let deferred_id = external(&mut core, "Std.Deferred.Deferred");
    let deferred = core.enum_type(deferred_id, vec![TypeId::ANY]);
    let child_id = external(&mut core, "Std.Fiber.Fiber");
    let child = core.enum_type(child_id, vec![TypeId::ANY]);
    let parameter = core.types.intern(Type::Param(0));
    let option_id = core.add_enum(EnumDef {
        name: "Option".into(),
        params: 1,
        variants: vec![
            VariantDef::positional("Some", vec![parameter]),
            VariantDef::unit("None"),
        ],
        error: false,
        external: false,
    });
    let option = core.enum_type(option_id, vec![TypeId::ANY]);
    let scope_fn = core.types.intern(Type::Fn(FnType {
        suspends: true,
        ..FnType::pure(vec![TypeId::FIBER_SCOPE], TypeId::ANY)
    }));
    let child_fn = core.types.intern(Type::Fn(FnType {
        suspends: true,
        ..FnType::pure(vec![], TypeId::ANY)
    }));
    let cell = core.types.intern(Type::Cell(TypeId::ANY));
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "make");
    b.set_exported().set_ret(TypeId::ANY);
    let text = b.param("text", TypeId::STRING);
    let parts = b.assign(
        "parts",
        list,
        Value::List {
            elems: vec![text.into()],
            tail: None,
        },
    );
    let caught = b.var("caught", TypeId::ANY);
    b.block(Some(caught), "caught", |b, label| {
        let error = b.assign(
            "error",
            error,
            Value::Make {
                ctor: CtorRef {
                    enum_id: error_id,
                    variant: 0,
                },
                args: vec![text.into(), parts.into()],
            },
        );
        b.term(Term::Break {
            label,
            value: Some(error.into()),
        });
    });
    b.term(Term::Return(Some(caught.into())));
    flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "plain");
    b.set_exported().set_ret(TypeId::ANY);
    let number = b.assign("number", TypeId::INT, Value::Const(Const::Int(42)));
    let value = b.assign("value", TypeId::ANY, Value::Coerce(number));
    b.term(Term::Return(Some(value.into())));
    flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "inspect");
    b.set_exported().set_ret(TypeId::INT);
    let input = b.param("input", TypeId::ANY);
    let is_error = b.assign(
        "is_error",
        TypeId::BOOL,
        Value::IsError {
            error: input,
            enum_id: error_id,
        },
    );
    let out = b.var("out", TypeId::INT);
    b.if_(
        Some(out),
        is_error,
        |b| {
            let value = b.assign(
                "error",
                error,
                Value::AsError {
                    error: input,
                    enum_id: error_id,
                },
            );
            let text = b.assign(
                "text",
                TypeId::STRING,
                Value::Field {
                    base: value,
                    ctor: CtorRef {
                        enum_id: error_id,
                        variant: 0,
                    },
                    index: 0,
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
            b.term(Term::Yield(Some(length.into())));
        },
        |b| {
            let value = b.assign("number", TypeId::INT, Value::Coerce(input));
            b.term(Term::Yield(Some(value.into())));
        },
    );
    b.term(Term::Return(Some(out.into())));
    flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "work");
    b.set_exported().set_suspends().set_ret(TypeId::ANY);
    let input = b.param("input", TypeId::ANY);
    let retry = b.param("retry", TypeId::ANY);
    let mode = b.param("mode", TypeId::INT);
    let scope = b.var("scope", TypeId::FIBER_SCOPE);
    let body = b.var("body", scope_fn);
    b.closure(body, vec![scope], vec![input, retry, mode], true, |b| {
        let mut cases = vec![];
        for route in 0..6 {
            let sequence = b.seq(|b| {
                if route == 2 || route == 3 {
                    let callback = b.var("callback", child_fn);
                    b.closure(callback, vec![], vec![input], true, |b| {
                        b.term(Term::Return(Some(input.into())))
                    });
                    let handle = b.assign(
                        "handle",
                        TypeId::ANY,
                        Value::Fiber {
                            op: emela_core::FiberOp::Fork,
                            args: vec![scope.into(), callback.into()],
                            on_fail: None,
                        },
                    );
                    let handle = b.assign("child", child, Value::Coerce(handle));
                    b.let_(
                        None,
                        Value::Fiber {
                            op: emela_core::FiberOp::Join,
                            args: vec![handle.into()],
                            on_fail: None,
                        },
                    );
                    let first = b.assign(
                        "first",
                        TypeId::ANY,
                        Value::Fiber {
                            op: emela_core::FiberOp::Join,
                            args: vec![handle.into()],
                            on_fail: None,
                        },
                    );
                    let second = b.assign(
                        "second",
                        TypeId::ANY,
                        Value::Fiber {
                            op: emela_core::FiberOp::Join,
                            args: vec![handle.into()],
                            on_fail: None,
                        },
                    );
                    b.term(Term::Yield(Some(
                        if route == 2 { first } else { second }.into(),
                    )));
                } else {
                    let raw = b.assign(
                        "raw",
                        TypeId::ANY,
                        Value::Fiber {
                            op: emela_core::FiberOp::Deferred,
                            args: vec![scope.into()],
                            on_fail: None,
                        },
                    );
                    let handle = b.assign("deferred", deferred, Value::Coerce(raw));
                    if route == 1 {
                        let callback = b.var("complete", child_fn);
                        b.closure(callback, vec![], vec![handle, input], true, |b| {
                            b.assign(
                                "completed",
                                TypeId::BOOL,
                                Value::Fiber {
                                    op: emela_core::FiberOp::Complete,
                                    args: vec![handle.into(), input.into()],
                                    on_fail: None,
                                },
                            );
                            b.term(Term::Return(Some(input.into())));
                        });
                        b.let_(
                            None,
                            Value::Fiber {
                                op: emela_core::FiberOp::Fork,
                                args: vec![scope.into(), callback.into()],
                                on_fail: None,
                            },
                        );
                    } else {
                        b.assign(
                            "first",
                            TypeId::BOOL,
                            Value::Fiber {
                                op: emela_core::FiberOp::Complete,
                                args: vec![handle.into(), input.into()],
                                on_fail: None,
                            },
                        );
                        b.assign(
                            "again",
                            TypeId::BOOL,
                            Value::Fiber {
                                op: emela_core::FiberOp::Complete,
                                args: vec![handle.into(), retry.into()],
                                on_fail: None,
                            },
                        );
                    }
                    if route == 5 {
                        b.term(Term::Yield(Some(input.into())));
                    } else if route == 4 {
                        let polled = b.assign(
                            "polled",
                            option,
                            Value::Fiber {
                                op: emela_core::FiberOp::Poll,
                                args: vec![handle.into()],
                                on_fail: None,
                            },
                        );
                        let first = b.assign(
                            "first",
                            TypeId::ANY,
                            Value::Field {
                                base: polled,
                                ctor: CtorRef {
                                    enum_id: option_id,
                                    variant: 0,
                                },
                                index: 0,
                            },
                        );
                        b.let_(
                            None,
                            Value::Fiber {
                                op: emela_core::FiberOp::DeferredAwait,
                                args: vec![handle.into()],
                                on_fail: None,
                            },
                        );
                        b.term(Term::Yield(Some(first.into())));
                    } else {
                        let got = b.assign(
                            "got",
                            TypeId::ANY,
                            Value::Fiber {
                                op: emela_core::FiberOp::DeferredAwait,
                                args: vec![handle.into()],
                                on_fail: None,
                            },
                        );
                        b.term(Term::Yield(Some(got.into())));
                    }
                }
            });
            cases.push((CaseKey::Lit(Const::Int(route)), sequence));
        }
        let out = b.var("out", TypeId::ANY);
        let invalid = b.seq(|b| b.term(Term::Unreachable));
        b.switch(Some(out), SwitchOn::Lit(mode), cases, Some(invalid));
        b.term(Term::Return(Some(out.into())));
    });
    let result = b.assign(
        "result",
        TypeId::ANY,
        Value::Fiber {
            op: emela_core::FiberOp::Scope { fails: false },
            args: vec![body.into()],
            on_fail: None,
        },
    );
    b.term(Term::Return(Some(result.into())));
    flow.add(b.finish());
    if gc {
        let mut b = FnBuilder::new(&flow.core, "copy");
        b.set_exported().set_ret(TypeId::ANY);
        let input = b.param("input", TypeId::ANY);
        let value = b.assign(
            "cell",
            cell,
            Value::Builtin {
                op: Builtin::CellNew,
                args: vec![input.into()],
            },
        );
        let result = b.assign(
            "result",
            TypeId::ANY,
            Value::Builtin {
                op: Builtin::CellGet,
                args: vec![value.into()],
            },
        );
        b.term(Term::Return(Some(result.into())));
        flow.add(b.finish());
    }
    let mut b = FnBuilder::new(&flow.core, "show_data_error");
    b.set_ret(TypeId::STRING);
    let _error = b.param("error", error);
    b.term(Term::Return(Some(
        Const::String("data error".into()).into(),
    )));
    let show = flow.add(b.finish());
    flow.error_shows.push((error_id, show));
    flow
}

#[test]
fn tagged_error_any_values_remain_normal_data_through_child_and_deferred_lifetimes() {
    for gc in [false, true] {
        let mut output = compile_flow(
            &fixture(gc),
            &Options::default()
                .with_heap_bytes(8192)
                .with_fiber_capacity(8)
                .with_frame_capacity(32)
                .with_frame_bytes(2048)
                .with_deferred_capacity(2),
        )
        .unwrap();
        let source = &mut output
            .files
            .iter_mut()
            .find(|f| f.path == "emela_embedded.c")
            .unwrap()
            .contents;
        assert_eq!(
            source.contains("static void emela_heap_collect_cycles("),
            gc
        );
        source.push_str(
            r#"
void test_any_release(EmelaAny value) { emela_any_release(value); }
void test_cancel(EmelaFiber f) { emela_fiber_get(f)->cancelled = 1; }
void test_add_notes(EmelaAny *value) {
  emela_error_ensure_notes(value);
  emela_error_notes *notes=(emela_error_notes *)value->notes;
  emela_error_note *note=emela_heap_allocate(sizeof(*note)+4,emela_heap_header(notes)->kind+1);
  note->next=NULL;note->length=4;
  for(unsigned i=0;i<4;i++)note->bytes[i]="note"[i];
  notes->head=notes->tail=note;
}
int test_note(EmelaAny value) {
  emela_error_notes *notes=(emela_error_notes *)value.notes;
  return notes && notes->head && notes->head->length==4 && notes->head->bytes[0]=='n';
}
int test_heap_empty(void) {
  for(unsigned char *p=emela_heap_storage;emela_heap_initialized && p<emela_heap_end();) {
    emela_heap_block *block=(emela_heap_block *)(void *)p;
    if(!block->is_free)return 0;
    p+=block->size;
  }
  return 1;
}
"#,
        );
        let deep_copy = if gc {
            "EmelaAny borrowed=error; borrowed.payload.e0.payload.v0.f0=(EmelaString){bytes,3,NULL}; EmelaAny copy=emela_export_copy_4(borrowed); test_any_release(error); error=copy;"
        } else {
            ""
        };
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
void test_any_release(EmelaAny);void test_cancel(EmelaFiber);void test_add_notes(EmelaAny *);int test_note(EmelaAny);int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n){{fprintf(stderr,"%d: %.*s\n",k,n,m);abort();}}
int main(void){{for(unsigned i=0;i<1000;i++){{
  char bytes[]={{'a',0,'b'}};EmelaAny error=emela_export_make_0((EmelaString){{bytes,3,NULL}});test_add_notes(&error);
  {deep_copy}
  memset(bytes,'x',3);EmelaAny plain=emela_export_plain_1();
  for(int tagged=0;tagged<2;tagged++)for(int mode=0;mode<6;mode++)for(int discard=0;discard<2;discard++){{
    EmelaAny input=tagged?error:plain,retry=tagged?plain:error,result={{0}};
    EmelaFiber f=emela_export_work_3_start(input,retry,mode,discard?NULL:&result);
    while(!emela_fiber_done(f)){{uint32_t steps=emela_scheduler_run(1);assert(steps || emela_fiber_done(f));}}
    emela_fiber_release(f);
    if(!discard){{assert(result.tag==input.tag);assert(emela_export_inspect_2(result)==(tagged?3:42));
      if(tagged){{assert(test_note(result));assert(result.payload.e0.payload.v0.f0.length==3 && memcmp(result.payload.e0.payload.v0.f0.data,"a\0b",3)==0);}}
      test_any_release(result);
    }}
  }}
  for(int mode=0;mode<6;mode++){{EmelaFiber f=emela_export_work_3_start(error,plain,mode,NULL);(void)emela_scheduler_run(1);test_cancel(f);while(!emela_fiber_done(f)){{uint32_t steps=emela_scheduler_run(1);assert(steps || emela_fiber_done(f));}}
    emela_fiber_release(f);}}
  test_any_release(error);test_any_release(plain);assert(test_heap_empty());
}}return 0;}}
"#
            ),
        );
    }
}
