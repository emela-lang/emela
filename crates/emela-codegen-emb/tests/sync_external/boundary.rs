use super::*;

use crate::gc_anchor;

fn entry(input: TypeId, result: TypeId, symbol: &str) -> Module {
    let mut flow = Module::new(emela_core::Module::new());
    let mut b = FnBuilder::new(&flow.core, "entry");
    b.set_exported().set_ret(result);
    b.param("value", input);
    flow.add(b.finish_external(binding(symbol)));
    flow
}

#[test]
fn adapter_symbols_and_signature_collisions_are_diagnosed_before_c_emission() {
    for target in [
        "invalid-symbol",
        "emela_const_probe",
        "emela_string_probe",
        "emela_any_probe",
        "emela_error_probe",
        "emela_cell_probe",
        "emela_export_entry_0",
    ] {
        let errors = compile_flow(
            &entry(TypeId::INT, TypeId::INT, "host"),
            &Options::default()
                .with_external_adapter("host", ExternalAdapter::new(target, ExternalResult::Owned)),
        )
        .unwrap_err();
        assert!(errors.iter().any(|e| {
            e.message
                .contains("conflicts with a generated declaration or is invalid")
        }));
    }
    let errors = compile_flow(
        &entry(TypeId::INT, TypeId::INT, "host"),
        &Options::default().with_external_adapter(
            "host",
            ExternalAdapter::new("board", ExternalResult::StringBuffer),
        ),
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("requires a String result"))
    );
    let errors = compile_flow(
        &entry(TypeId::STRING, TypeId::STRING, "host"),
        &Options::default().with_heap_bytes(1024),
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("non-scalar signature"))
    );
    for overrides in [false, true] {
        let mut flow = Module::new(emela_core::Module::new());
        for (name, ty, symbol) in [
            ("integer", TypeId::INT, "first"),
            ("real", TypeId::FLOAT, "second"),
        ] {
            let mut b = FnBuilder::new(&flow.core, name);
            b.set_exported().set_ret(ty);
            b.param("value", ty);
            flow.add(b.finish_external(binding(if overrides { symbol } else { "board_common" })));
        }
        let options = if overrides {
            Options::default()
                .with_external_adapter(
                    "first",
                    ExternalAdapter::new("board_common", ExternalResult::Owned),
                )
                .with_external_adapter(
                    "second",
                    ExternalAdapter::new("board_common", ExternalResult::Owned),
                )
        } else {
            Options::default()
        };
        let errors = compile_flow(&flow, &options).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("incompatible signatures"))
        );
    }
    let mut flow = entry(TypeId::INT, TypeId::INT, "host_sync");
    let mut b = FnBuilder::new(&flow.core, "task");
    b.set_exported().set_suspends().set_ret(TypeId::INT);
    b.param("value", TypeId::INT);
    flow.add(b.finish_external(binding("host_task")));
    let errors = compile_flow(
        &flow,
        &Options::default()
            .with_external_adapter(
                "host_sync",
                ExternalAdapter::new("board_common", ExternalResult::Owned),
            )
            .with_task_adapter("host_task", "board_common"),
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("incompatible synchronous and task"))
    );
}

#[test]
fn scalar_overrides_keep_isr_safety_and_float_bits_while_non_scalar_adapters_are_rejected() {
    let flow = entry(TypeId::FLOAT, TypeId::FLOAT, "host_scalar");
    let options = Options::default()
        .with_external_adapter(
            "host_scalar",
            ExternalAdapter::new("board_scalar", ExternalResult::Owned),
        )
        .with_isr_entry("entry");
    let errors = compile_flow(&flow, &options).unwrap_err();
    assert!(
        errors.iter().any(
            |e| e.message.contains("board_scalar") && e.message.contains("not marked ISR-safe")
        )
    );
    compile_and_run(
        compile_flow(&flow, &options.with_isr_safe_external("board_scalar")).unwrap(),
        r#"
#include <assert.h>
#include <string.h>
#include "emela_embedded.h"
double board_scalar(double value) {return value;}
int main(void) {static const uint64_t cases[]={0,UINT64_C(0x8000000000000000),UINT64_C(0x7ff0000000000000),UINT64_C(0xfff0000000000000),UINT64_C(0x7ff8000000000042),1,UINT64_C(0x7fefffffffffffff)};
 for(unsigned i=0;i<sizeof cases/sizeof *cases;i++) {double input;memcpy(&input,&cases[i],sizeof input);double output=emela_export_entry_0(input);uint64_t bits;memcpy(&bits,&output,sizeof bits);assert(bits==cases[i]);}}
"#,
    );
    let flow = entry(TypeId::STRING, TypeId::STRING, "host_string");
    for policy in [
        ExternalResult::Borrowed,
        ExternalResult::Owned,
        ExternalResult::StringBuffer,
    ] {
        let errors = compile_flow(
            &flow,
            &Options::default()
                .with_heap_bytes(1024)
                .with_isr_entry("entry")
                .with_isr_safe_external("board_string")
                .with_external_adapter("host_string", ExternalAdapter::new("board_string", policy)),
        )
        .unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("non-scalar external adapter")
                    || e.message.contains("heap-backed"))
        );
    }
}

#[test]
fn opaque_external_handles_roundtrip_without_emela_heap_or_ownership_changes() {
    let mut core = emela_core::Module::new();
    let opaque = core.add_enum(EnumDef {
        name: "Handle".into(),
        params: 0,
        variants: vec![],
        error: false,
        external: true,
    });
    let ty = core.enum_type(opaque, vec![]);
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "entry");
    b.set_exported().set_ret(ty);
    b.param("value", ty);
    flow.add(b.finish_external(binding("host_handle")));
    for policy in [ExternalResult::Borrowed, ExternalResult::Owned] {
        let output = compile_flow(
            &flow,
            &Options::default()
                .with_external_adapter("host_handle", ExternalAdapter::new("board_handle", policy)),
        )
        .unwrap();
        assert!(
            !output
                .files
                .iter()
                .any(|f| f.contents.contains("emela_heap_storage"))
        );
        compile_and_run(
            output,
            r#"
#include <assert.h>
#include <stddef.h>
#include "emela_embedded.h"
void *board_handle(void *handle) {return handle;}
int main(void) {int context=42;for(int i=0;i<1000;i++) {assert(emela_export_entry_0(&context)==&context);assert(emela_export_entry_0(NULL)==NULL);}}
"#,
        );
    }
}

#[test]
fn opaque_board_contexts_can_hold_owned_emela_fields_as_collector_roots() {
    let mut core = emela_core::Module::new();
    let id = core.add_enum(EnumDef {
        name: "Handle".into(),
        params: 0,
        variants: vec![],
        error: false,
        external: true,
    });
    let ty = core.enum_type(id, vec![]);
    let mut flow = Module::new(core);
    for (name, input, symbol) in [
        ("make", TypeId::STRING, "host_make"),
        ("echo", ty, "host_echo"),
    ] {
        let mut b = FnBuilder::new(&flow.core, name);
        b.set_exported().set_ret(ty);
        b.param("value", input);
        flow.add(b.finish_external(binding(symbol)));
    }
    for with_collector in [false, true] {
        let mut flow = flow.clone();
        if with_collector {
            gc_anchor::add(&mut flow);
        }
        let mut output = compile_flow(
            &flow,
            &Options::default()
                .with_heap_bytes(512)
                .with_external_adapter(
                    "host_make",
                    ExternalAdapter::new("board_make", ExternalResult::Owned),
                )
                .with_external_adapter(
                    "host_echo",
                    ExternalAdapter::new("board_echo", ExternalResult::Borrowed),
                ),
        )
        .unwrap();
        add_heap_check(&mut output);
        compile_and_run(
        output,
        &r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);typedef struct {EmelaString text;} Context;static Context context;
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) {(void)k;(void)m;(void)n;abort();}
void *board_make(EmelaString input) {assert(!context.text.owner);context.text=input;emela_external_own_4(&context.text);return &context;}
void *board_echo(void *input) {return input;}
int main(void) {for(int i=0;i<1000;i++) {char bytes[]={'a','\0','b'};Context *value=emela_export_make_0((EmelaString){.data=bytes,.length=3});memset(bytes,'x',3);
 GC_STEP();assert(emela_export_echo_1(value)==value);assert(value->text.owner&&value->text.length==3&&!memcmp(value->text.data,"a\0b",3));
 emela_external_release_4(&value->text);assert(test_heap_empty());}}
"#.replace("GC_STEP();", if with_collector { "(void)emela_export_gc_anchor_2((EmelaAny){0});" } else { "" }),
    );
    }
}
