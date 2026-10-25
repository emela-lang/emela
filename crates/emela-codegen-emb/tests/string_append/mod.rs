use super::*;

#[test]
fn literal_prefixes_are_copied_without_treating_readonly_bytes_as_writable_output() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
pub fn message(text: String) -> String {
  String.append(String.append("Missing(text: ", text), ")")
}
"#,
        )],
        Roots::Public,
    )
    .unwrap();
    let output = compile(&core, &Options::default().with_string_capacity(64)).unwrap();
    let source = &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents;
    let append = source
        .lines()
        .find(|line| line.contains("= emela_emb_string_append((EmelaString)"))
        .unwrap();
    assert!(append.ends_with(", INT32_C(1));"));
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
int main(void) {
  for (int i = 0; i < 1000; ++i) {
    const char input[] = {'a', 0, 'b'};
    char output[64];
    EmelaString text = emela_export_message_0((EmelaString){.data = input, .length = 3}, output, 64);
    assert(text.length == 18 && text.data == output && !text.owner);
    assert(memcmp(text.data, "Missing(text: a\0b)", 18) == 0);
  }
}
"#,
    );
}
