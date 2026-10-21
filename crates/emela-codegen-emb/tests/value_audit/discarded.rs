use super::*;

fn constructor_fixture(kind: &str) -> Module {
    let mut core = emela_core::Module::new();
    let effect = core.add_effect(emela_core::EffectDef::new("Empty", vec![]));
    let handler = core.handler(effect);
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "handler");
    b.set_exported().set_ret(handler);
    let value = b.assign(
        "value",
        handler,
        Value::Handler {
            name: "Empty".into(),
            effect,
            ops: vec![],
        },
    );
    b.term(Term::Return(Some(value.into())));
    flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "work");
    b.set_exported();
    let text = if kind == "list" {
        Some(b.param("text", TypeId::STRING))
    } else {
        None
    };
    let parameter = if kind == "extend" {
        Some(b.param("handler", handler))
    } else {
        None
    };
    for _ in 0..13 {
        b.let_(
            None,
            match kind {
                "list" => Value::List {
                    elems: vec![text.unwrap().into()],
                    tail: None,
                },
                "handler" => Value::Handler {
                    name: "Empty".into(),
                    effect,
                    ops: vec![],
                },
                "extend" => Value::EvExtend {
                    base: None,
                    bindings: vec![(effect, parameter.unwrap())],
                },
                _ => unreachable!(),
            },
        );
    }
    b.term(Term::Return(None));
    flow.add(b.finish());
    flow
}

fn run_constructor(kind: &str) {
    let flow = constructor_fixture(kind);
    emela_lir::verify(&flow).unwrap();
    let mut output = compile_flow(&flow, &Options::default().with_heap_bytes(512)).unwrap();
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
    emela_heap_block *b=(emela_heap_block *)(void *)p;
    if(!b->is_free) return 0;
    p+=b->size;
  }
  return 1;
}
"#,
        );
    let call = match kind {
        "list" => "char bytes[]={'a',0,'b'}; emela_export_work_1((EmelaString){bytes,3,NULL});",
        "handler" => "emela_export_work_1();",
        "extend" => {
            "void *handler=emela_export_handler_0(); emela_export_work_1(handler); emela_embedded_release(handler);"
        }
        _ => unreachable!(),
    };
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
int main(void) {{ for(unsigned i=0;i<1000;i++) {{ {call} assert(test_heap_empty()); }} return 0; }}
"#
        ),
    );
}

#[test]
fn discarded_lists_are_allocated_and_released_before_the_next_statement() {
    run_constructor("list");
}

#[test]
fn discarded_empty_lists_remain_heapless_and_nonempty_lists_preserve_exhaustion() {
    let core = emela_core::Module::new();
    let mut b = FnBuilder::new(&core, "empty");
    b.set_exported();
    b.let_(
        None,
        Value::List {
            elems: vec![],
            tail: None,
        },
    );
    b.term(Term::Return(None));
    let function = b.finish();
    let mut flow = Module::new(core);
    flow.add(function);
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
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\nint main(void) { emela_export_empty_0(); return 0; }\n",
    );
    let output = compile_flow(
        &constructor_fixture("list"),
        &Options::default().with_heap_bytes(64),
    )
    .unwrap();
    compile_and_run(
        output,
        r#"
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *s,uint16_t n) { (void)s;(void)n;exit(k==EMELA_DEFECT_HEAP_EXHAUSTED?0:2); }
int main(void) { emela_export_work_1((EmelaString){"abc",3,NULL}); return 1; }
"#,
    );
}

#[test]
fn discarded_handlers_are_allocated_and_released_before_the_next_statement() {
    run_constructor("handler");
}

#[test]
fn discarded_evidence_extensions_release_their_handler_references_immediately() {
    run_constructor("extend");
}

fn lookup_fixture(discard: bool, found: bool) -> Module {
    let mut core = emela_core::Module::new();
    let effect = core.add_effect(emela_core::EffectDef::new("Empty", vec![]));
    let handler = core.handler(effect);
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "run");
    b.set_exported();
    let (base, bindings) = if found {
        let value = b.assign(
            "handler",
            handler,
            Value::Handler {
                name: "Empty".into(),
                effect,
                ops: vec![],
            },
        );
        (None, vec![(effect, value)])
    } else {
        (None, vec![])
    };
    let evidence = b.assign(
        "evidence",
        TypeId::EVIDENCE,
        Value::EvExtend { base, bindings },
    );
    let result = (!discard).then(|| b.var("unused", handler));
    b.let_(
        result,
        Value::EvLookup {
            ev: evidence,
            effect,
        },
    );
    b.term(Term::Return(None));
    flow.add(b.finish());
    flow
}

#[test]
fn unused_evidence_lookup_preserves_missing_handler_defects_with_and_without_a_destination() {
    for discard in [false, true] {
        let flow = lookup_fixture(discard, false);
        emela_lir::verify(&flow).unwrap();
        let output = compile_flow(&flow, &Options::default().with_heap_bytes(1024)).unwrap();
        compile_and_run(
            output,
            r#"
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *s,uint16_t n) { exit(k==EMELA_DEFECT_PANIC && n==23 && memcmp(s,"effect handler not found",23)==0?0:2); }
int main(void) { emela_export_run_0(); return 1; }
"#,
        );
    }
}

#[test]
fn statically_resolved_unused_evidence_lookup_stays_valid_with_and_without_a_destination() {
    for discard in [false, true] {
        let flow = lookup_fixture(discard, true);
        emela_lir::verify(&flow).unwrap();
        let output = compile_flow(&flow, &Options::default().with_heap_bytes(1024)).unwrap();
        compile_and_run(
            output,
            r#"
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *s,uint16_t n) { (void)k; (void)s; (void)n; abort(); }
int main(void) { for(unsigned i=0;i<1000;i++) emela_export_run_0(); return 0; }
"#,
        );
    }
}

#[test]
fn unused_dynamic_evidence_lookup_keeps_validation_and_the_callers_owner() {
    for discard in [false, true] {
        let mut core = emela_core::Module::new();
        let effect = core.add_effect(emela_core::EffectDef::new("Empty", vec![]));
        let handler = core.handler(effect);
        let mut flow = Module::new(core);
        let mut b = FnBuilder::new(&flow.core, "make");
        b.set_exported().set_ret(TypeId::EVIDENCE);
        let h = b.assign(
            "handler",
            handler,
            Value::Handler {
                name: "Empty".into(),
                effect,
                ops: vec![],
            },
        );
        let ev = b.assign(
            "evidence",
            TypeId::EVIDENCE,
            Value::EvExtend {
                base: None,
                bindings: vec![(effect, h)],
            },
        );
        b.term(Term::Return(Some(ev.into())));
        flow.add(b.finish());
        let mut b = FnBuilder::new(&flow.core, "run");
        b.set_exported();
        let ev = b.param("evidence", TypeId::EVIDENCE);
        let result = (!discard).then(|| b.var("unused", handler));
        b.let_(result, Value::EvLookup { ev, effect });
        b.term(Term::Return(None));
        flow.add(b.finish());
        emela_lir::verify(&flow).unwrap();
        let mut output = compile_flow(&flow, &Options::default().with_heap_bytes(1024)).unwrap();
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
    emela_heap_block *b=(emela_heap_block *)(void *)p;
    if(!b->is_free) return 0;
    p+=b->size;
  }
  return 1;
}
"#,
            );
        compile_and_run(
            output,
            r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *s,uint16_t n) { exit(k==EMELA_DEFECT_PANIC && n==23 && memcmp(s,"effect handler not found",23)==0?0:2); }
int main(void) {
  for(unsigned i=0;i<1000;i++) {
    void *evidence=emela_export_make_0();
    for(unsigned n=0;n<13;n++) emela_export_run_1(evidence);
    emela_embedded_release(evidence); assert(test_heap_empty());
  }
  emela_export_run_1(NULL); return 1;
}
"#,
        );
    }
}
