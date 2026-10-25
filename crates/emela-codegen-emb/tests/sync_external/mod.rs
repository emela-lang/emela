use super::*;
use emela_codegen_emb::{ExternalAdapter, ExternalResult};

mod boundary;
mod buffers;
mod entries;
mod failure;
mod values;

fn binding(symbol: &str) -> External {
    External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global(symbol.into()),
        }],
    }
}

#[test]
fn exported_native_external_entries_bridge_the_public_emela_abi() {
    let mut flow = Module::new(emela_core::Module::new());
    let mut b = FnBuilder::new(&flow.core, "entry");
    b.set_exported().set_ret(TypeId::INT);
    b.param("n", TypeId::INT);
    flow.add(b.finish_external(binding("board_entry")));
    let output =
        compile_flow(&flow, &Options::default()).expect("exported external entry has an adapter");
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
int32_t board_entry(int32_t n) { return n; }
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) { (void)k;(void)m;(void)n;abort(); }
int main(void) { assert(emela_export_entry_0(INT32_MIN)==INT32_MIN); }
"#,
    );
}

#[test]
fn source_string_adapters_snapshot_borrowed_results_and_board_held_arguments() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
@external(c,"host_echo")
fn echo(s: String) -> String use {}
pub fn direct(s: String) -> String { echo(s) }
pub fn indirect(s: String) -> String { f = echo
 f(s) }
pub fn discard(s: String) -> () { echo(s)
 () }
"#,
        )],
        Roots::Public,
    )
    .unwrap();
    let mut output = compile(
        &core,
        &Options::default()
            .with_heap_bytes(1024)
            .with_external_adapter(
                "host_echo",
                ExternalAdapter::new("board_echo", ExternalResult::Borrowed),
            ),
    )
    .unwrap();
    add_heap_check(&mut output);
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);static EmelaString held;
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) { (void)k;(void)m;(void)n;abort(); }
EmelaString board_echo(EmelaString value) { held=value;emela_external_own_4(&held);return value; }
int main(void) { for(int i=0;i<1000;i++) {
 char bytes[]={'a','\0','b'};EmelaString input={.data=bytes,.length=3};
 EmelaString value=emela_export_direct_0(input);memset(bytes,'x',3);
 assert(value.owner&&value.length==3&&!memcmp(value.data,"a\0b",3));assert(held.owner&&!memcmp(held.data,"a\0b",3));
 emela_embedded_release_string(&value);emela_external_release_4(&held);assert(test_heap_empty());
 value=emela_export_indirect_1(input);emela_embedded_release_string(&value);emela_external_release_4(&held);assert(test_heap_empty());
 emela_export_discard_2(input);emela_external_release_4(&held);assert(test_heap_empty());
} }
"#,
    );
}

fn add_heap_check(output: &mut Output) {
    output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents
        .push_str(
            r#"
int test_heap_empty(void) {
 if(!emela_heap_initialized)return 1;
 for(unsigned char*p=emela_heap_storage;p<emela_heap_end();) {
  emela_heap_block*b=(emela_heap_block*)(void*)p;if(!b->is_free)return 0;p+=b->size;
 }return 1;
}
"#,
        );
}
