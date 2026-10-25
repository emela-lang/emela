use super::*;
use emela_codegen_emb::{ExternalAdapter, ExternalResult};

fn heap_probe(output: &mut Output, gc: bool) {
    let source = &mut output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents;
    source.push_str("size_t const_external_heap(void) {\n");
    if gc {
        source.push_str("emela_heap_collect_cycles();\n");
    }
    source.push_str("size_t used=0; if(!emela_heap_initialized)return 0; for(unsigned char*p=emela_heap_storage;p<emela_heap_end();) { emela_heap_block*b=(emela_heap_block*)(void*)p; if(!b->is_free)used+=b->size;p+=b->size;}return used;}\n");
    source.push_str("EmelaString const_external_owned(EmelaString value) { return emela_heap_copy_string(value); }\n");
}

#[test]
fn pure_external_string_const_caches_borrowed_owned_and_caller_buffer_values_across_public_flow_and_gc()
 {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
@external(c,"host_value")
fn value() -> String use {}
const VALUE = value()
pub fn text() -> String { VALUE }
pub fn size() -> Int { String.byte_size(VALUE) }
pub fn discard() { VALUE
 () }
"#,
        )],
        Roots::Public,
    )
    .unwrap();
    for policy in [
        ExternalResult::Borrowed,
        ExternalResult::Owned,
        ExternalResult::StringBuffer,
    ] {
        for gc in [false, true] {
            for via_flow in [false, true] {
                let options = Options::default()
                    .with_heap_bytes(2048)
                    .with_external_adapter(
                        "host_value",
                        ExternalAdapter::new("board_value", policy),
                    );
                let mut output = if gc || via_flow {
                    let mut flow = emela_codegen_emb::flow_from_core(&core).unwrap();
                    if gc {
                        gc_anchor::add(&mut flow);
                    }
                    compile_flow(&flow, &options).unwrap()
                } else {
                    compile(&core, &options).unwrap()
                };
                let text = export(&output, "text");
                let size = export(&output, "size");
                let discard = export(&output, "discard");
                heap_probe(&mut output, gc);
                let board = match policy {
                    ExternalResult::Borrowed => {
                        "EmelaString board_value(void) { calls++; return (EmelaString){.data=bytes,.length=3}; }"
                    }
                    ExternalResult::Owned => {
                        "EmelaString board_value(void) { calls++; return const_external_owned((EmelaString){.data=bytes,.length=3}); }"
                    }
                    ExternalResult::StringBuffer => {
                        "EmelaString board_value(char *out,uint16_t capacity) { calls++; if(capacity<3)abort();memcpy(out,bytes,3);return (EmelaString){.data=out,.length=3}; }"
                    }
                };
                compile_and_run(
                    output,
                    &format!(
                        r#"{BOARD}
static char bytes[]={{'4','\0','2'}};
static int calls;
size_t const_external_heap(void);
EmelaString const_external_owned(EmelaString value);
{board}
int main(void) {{
 {discard}(); memset(bytes,'x',3); size_t baseline=const_external_heap(); if(!baseline || calls!=1)return 1;
 for(int i=0;i<1000;i++) {{ EmelaString value={text}();if(value.length!=3||!value.owner||memcmp(value.data,"4\0" "2",3))return 2;emela_embedded_release_string(&value);if({size}()!=3||const_external_heap()!=baseline)return 3; }}
 return calls==1?0:4;
}}
"#
                    ),
                );
            }
        }
    }
}

#[test]
fn pure_external_opaque_const_memoizes_the_board_handle_without_heap() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
@external
type Token
@external(c,"host_token")
fn token() -> Token use {}
const TOKEN = token()
pub fn read() -> Token { TOKEN }
"#,
        )],
        Roots::Public,
    )
    .unwrap();
    for policy in [ExternalResult::Borrowed, ExternalResult::Owned] {
        for null_result in [false, true] {
            let output = compile(
                &core,
                &Options::default().with_external_adapter(
                    "host_token",
                    ExternalAdapter::new("board_token", policy),
                ),
            )
            .unwrap();
            assert!(
                !output
                    .files
                    .iter()
                    .find(|f| f.path == "emela_embedded.c")
                    .unwrap()
                    .contents
                    .contains("emela_heap_storage")
            );
            let read = export(&output, "read");
            let pointer = if null_result { "NULL" } else { "&token" };
            compile_and_run(
                output,
                &format!(
                    r#"{BOARD}
static int calls, token=42;
void *board_token(void) {{ calls++;(void)token;return {pointer}; }}
int main(void) {{ for(int i=0;i<1000;i++)if({read}()!={pointer})return 1;return calls==1?0:2; }}
"#
                ),
            );
        }
    }
}

#[test]
fn pure_external_aggregate_const_owns_string_and_list_fields_after_board_release() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
@external(c,"host_packet")
fn packet() -> (String,List[String]) use {}
const VALUE = packet()
pub fn read() -> (String,List[String]) { VALUE }
pub fn make(value: String) -> (String,List[String]) { (value,[value]) }
pub fn first() -> String { (_,values)=VALUE
 values match { [head,..tail] -> head
 _ -> "" } }
"#,
        )],
        Roots::Public,
    )
    .unwrap();
    for gc in [false, true] {
        let mut flow = emela_codegen_emb::flow_from_core(&core).unwrap();
        if gc {
            gc_anchor::add(&mut flow);
        }
        let mut output = compile_flow(
            &flow,
            &Options::default()
                .with_heap_bytes(4096)
                .with_external_adapter(
                    "host_packet",
                    ExternalAdapter::new("board_packet", ExternalResult::Borrowed),
                ),
        )
        .unwrap();
        let read = export(&output, "read");
        let make = export(&output, "make");
        let first = export(&output, "first");
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
            .unwrap()
            .to_owned();
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
            .unwrap()
            .to_owned();
        heap_probe(&mut output, gc);
        compile_and_run(
            output,
            &format!(
                r#"{BOARD}
static int calls;static char bytes[]={{'a','\0','b'}};static {cty} held;
size_t const_external_heap(void);
{cty} board_packet(void) {{ calls++;held={make}((EmelaString){{.data=bytes,.length=3}});return held; }}
int main(void) {{
 {cty} value={read}();{release}(&value);{release}(&held);memset(bytes,'x',3);size_t baseline=const_external_heap();
 for(int i=0;i<1000;i++) {{value={read}();if(value.f0.length!=3||memcmp(value.f0.data,"a\0b",3))return 1;{release}(&value);EmelaString text={first}();if(text.length!=3||memcmp(text.data,"a\0b",3))return 2;emela_embedded_release_string(&text);if(const_external_heap()!=baseline)return 3;}}
 return calls==1?0:4;
}}
"#
            ),
        );
    }
}

#[test]
fn adapter_contract_defects_are_memoized_with_the_source_const_name() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
@external(c,"host_value")
fn value() -> String use {}
const VALUE = value()
pub fn size() -> Int { String.byte_size(VALUE) }
"#,
        )],
        Roots::Public,
    )
    .unwrap();
    let output = compile(
        &core,
        &Options::default()
            .with_heap_bytes(1024)
            .with_string_capacity(16)
            .with_external_adapter(
                "host_value",
                ExternalAdapter::new("board_value", ExternalResult::StringBuffer),
            ),
    )
    .unwrap();
    let size = export(&output, "size");
    compile_and_run(
        output,
        &format!(
            r#"
#include <setjmp.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static jmp_buf target;static int calls;
EmelaString board_value(char *output,uint16_t capacity) {{calls++;return (EmelaString){{.data=output,.length=(uint16_t)(capacity+1)}};}}
_Noreturn void emela_board_defect(EmelaDefectKind kind,const char *bytes,uint16_t length) {{
 static const char expected[]="while evaluating const VALUE: invalid external String buffer";
 if(kind!=EMELA_DEFECT_STRING_CAPACITY||length!=sizeof expected-1||memcmp(bytes,expected,length))abort();
 longjmp(target,1);
}}
int main(void) {{for(volatile int i=0;i<1000;i++)if(setjmp(target)==0){{(void){size}();return 1;}}return calls==1?0:2;}}
"#
        ),
    );
}
