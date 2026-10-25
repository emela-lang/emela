use super::*;

#[test]
fn nested_generic_error_payloads_have_finite_display_functions() {
    let (definition, value, expected) = (
        "enum Wrap[A] { Box(List[A]) } derive Show",
        "Box([42])",
        "error: Boom(value: Box([42]))",
    );
    let source = format!(
        r#"
import Std.Fiber.{{Fiber, Success, Failure}}
import Std.Cause.{{Cause}}
{definition}
error Boom(value: {ty}) derive Show
fn bad() -> Int fails Boom {{fail Boom(value: {value})}}
pub fn show() -> String {{
  Fiber.attempt(bad) match {{Success(_) -> "unexpected"
    Failure(cause) -> Cause.to_string(cause)}}
}}
"#,
        ty = "Wrap[Int]"
    );
    let core = support::try_lower(&[("main.emel", &source)], Roots::Public).unwrap();
    let flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    assert!(flow.functions.len() < 64);
    assert_eq!(flow.error_shows.len(), 1);
    let mut output = compile_flow(
        &flow,
        &Options::default()
            .with_heap_bytes(2048)
            .with_string_capacity(128),
    )
    .unwrap();
    let show = export_name(&output, "show");
    add_heap_check(&mut output);
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *s,uint16_t n) {{(void)k;(void)s;(void)n;abort();}}
int main(void) {{for(int i=0;i<1000;i++) {{char buffer[128];EmelaString result={show}(buffer,sizeof buffer);assert(result.length=={length} && memcmp(result.data,"{expected}",{length})==0);assert(test_heap_empty());}}}}
"#,
            length = expected.len()
        ),
    );
}
