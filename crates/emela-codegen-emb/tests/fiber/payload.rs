use super::*;

pub(super) fn payload_notes_flow() -> Module {
    let core = support::try_lower(
        &[(
            "main.emel",
            include_str!("../../examples/support/fiber_payload.emel"),
        )],
        Roots::Exported,
    )
    .unwrap();
    let mut flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    let (_, late) = flow
        .functions
        .iter_mut()
        .find(|(_, f)| f.name == "late")
        .unwrap();
    // Keep all siblings masked until they have reported their failures.
    let hold = late.stmts.alloc(emela_lir::Stmt::Let {
        dst: None,
        on_defect: None,
        on_suspend: None,
        value: Value::Runtime {
            op: RuntimeOp::Hold,
            args: vec![],
        },
    });
    late.seqs[late.body.unwrap()].stmts.insert(0, hold);
    flow
}

#[test]
fn operation_failure_before_resume_transfers_and_releases_its_string_payload() {
    let source = r#"
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
error Boom(message: String) derive Show
effect Wait { suspend fn get(n: Int) -> Int fails Boom }
handler Waiting: Wait {
  suspend fn get(n, resume) {
    value = wait(n)
    if n == 0 { resume(value) } else { fail Boom(message: Int.to_string(value)) }
  }
}
pub fn caught(n: Int) -> Int use Async {
  with Waiting {
    (use Wait).get(n) escape { Boom(message) -> String.byte_size(message) }
  }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    let mut output = compile(&core, &options().with_heap_bytes(4096)).unwrap();
    notes::add_heap_check(&mut output);
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask task;
int test_heap_empty(void);
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t, int32_t n) { (void)n; task = t; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k,n,m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; ++i) {
    int32_t result = -1;
    EmelaFiber f = emela_export_caught_0_start(i % 2, &result);
    while (emela_scheduler_run(1)) {}
    assert(emela_task_complete_int(task, INT32_MIN));
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
    assert(result == (i % 2 ? 11 : INT32_MIN));
    assert(test_heap_empty());
  }
}
"#,
    );
}

#[test]
fn scope_causes_and_notes_keep_string_payloads_after_frames_and_scopes_end() {
    let mut output = compile_flow(
        &payload_notes_flow(),
        &options()
            .with_heap_bytes(4096)
            .with_frame_capacity(48)
            .with_frame_bytes(768)
            .with_task_capacity(4)
            .with_event_capacity(4),
    )
    .unwrap();
    notes::add_heap_check(&mut output);
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
static EmelaTask tasks[4];
static int count;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t, int32_t n) { assert(n >= 1 && n <= 4); tasks[n-1] = t; count++; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k,n,m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; ++i) {
    void *cause = NULL; count = 0;
    EmelaFiber f = emela_export_work_0_start(i % 3 ? &cause : NULL);
    while (emela_scheduler_run(1)) {}
    assert(count == 4);
    int order[3] = {1, 2, 3}; if (i % 2) { order[0] = 3; order[2] = 1; }
    for (int n = 0; n < 3; ++n) {
      assert(emela_task_complete_int(tasks[order[n]-1], 0));
      while (emela_scheduler_run(1)) {}
    }
    assert(emela_fiber_done(f) && !emela_task_complete_int(tasks[3], 0));
    emela_fiber_release(f);
    if (i % 3) {
      void *again = NULL;
      if (i % 2) {
        EmelaFiber g = emela_export_rethrow_async_2_start(cause, &again);
        while (!emela_fiber_done(g)) assert(emela_scheduler_run(1));
        emela_fiber_release(g);
      } else again = emela_export_rethrow_1(cause);
      emela_embedded_release(cause);
      char buffer[256], expected[256];
      int length = snprintf(expected, sizeof expected, "error: Boom(message: \"%d\")\nnote: another fiber also failed: error: Boom(message: \"2\")\nnote: another fiber also failed: error: Boom(message: \"%d\")", order[0], order[2]);
      EmelaString shown = emela_export_show_3(again, buffer, sizeof buffer);
      assert(shown.length == length && memcmp(shown.data, expected, length) == 0);
      EmelaString payload = emela_export_caught_4(again);
      emela_embedded_release(again);
      assert(payload.owner && payload.length == 1 && payload.data[0] == '0' + order[0]);
      emela_embedded_release_string(&payload);
    }
    assert(test_heap_empty());
  }
}
"#,
    );
}

#[test]
fn native_failure_channels_own_formatted_parse_errors_and_list_payloads() {
    let core = support::try_lower(
        &[(
            "main.emel",
            include_str!("../../examples/support/error_payload_native.emel"),
        )],
        Roots::Exported,
    )
    .unwrap();
    let mut output = compile(&core, &options().with_heap_bytes(2048)).unwrap();
    notes::add_heap_check(&mut output);
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static char *input;
static int cleanups;
int test_heap_empty(void);
void mutate(void) { memset(input, 'x', 5); cleanups++; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k,n,m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; ++i) {
    assert(emela_export_caught_0(INT32_MIN) == 12);
    assert(emela_export_batch_1(INT32_MIN) == 12);
    assert(emela_export_local_4(INT32_MIN) == 11);
    char bytes[] = {'a', 0, 'b', (char)0xc3, (char)0xa9}; input = bytes;
    EmelaString result = emela_export_snapshot_2((EmelaString){.data = bytes, .length = 5});
    assert(cleanups == i * 2 + 1 && memcmp(bytes, "xxxxx", 5) == 0);
    assert(result.owner && result.length == 5 && memcmp(result.data, "a\0b\xc3\xa9", 5) == 0);
    emela_embedded_release_string(&result);
    memcpy(bytes, "abcde", 5);
    assert(emela_export_captured_3((EmelaString){.data = bytes, .length = 5}) == 42);
    assert(cleanups == i * 2 + 2);
    assert(test_heap_empty());
  }
}

"#,
    );
}

#[test]
fn list_error_payloads_survive_suspending_cleanup_and_are_dropped_on_cancellation() {
    let source = r#"
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
error Batch(items: List[String]) derive Show
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting: Wait { suspend fn get(n, resume) { resume(wait(n)) } }
effect Resource { fn read() -> Int }
handler Res: Resource {
  fn read() { 0 }
  release { (use Wait).get(2) }
}
fn fail_batch(n: Int) -> Int fails Batch {
  fail Batch(items: [Int.to_string(n), "tail"])
}
fn failing(n: Int) -> Int fails Batch use Wait {
  with Res {
    (use Wait).get(1)
    fail_batch(n)
  }
}
pub fn caught(n: Int, direct: Bool) -> Int use Async {
  with Waiting {
    { if direct {
      with Res {
        (use Wait).get(1)
        fail Batch(items: [Int.to_string(n), "tail"])
      }
    } else { failing(n) } } escape { Batch(items) -> {
      (use Wait).get(3)
      items match {
        [text, ..rest] -> String.byte_size(text) + List.length(rest)
        [] -> -1
      }
    } }
  }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    let mut output = compile(&core, &options().with_heap_bytes(4096)).unwrap();
    notes::add_heap_check(&mut output);
    output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents
        .push_str("void test_cancel(EmelaFiber h) { emela_fiber_get(h)->cancelled = 1; }\n");
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
void test_cancel(EmelaFiber h);
static EmelaTask task;
static int waits;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t, int32_t n) { assert(n == ++waits); task = t; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k,n,m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; ++i) {
    int32_t result = -1; waits = 0;
    EmelaFiber f = emela_export_caught_0_start(INT32_MIN, (i / 2) % 2, &result);
    while (emela_scheduler_run(1)) {} assert(waits == 1);
    assert(emela_task_complete_int(task, 0));
    while (emela_scheduler_run(1)) {} assert(waits == 2 && result == -1);
    assert(emela_task_complete_int(task, 0));
    while (emela_scheduler_run(1)) {} assert(waits == 3 && result == -1);
    if (i % 2) test_cancel(f);
    else assert(emela_task_complete_int(task, 0));
    while (!emela_fiber_done(f)) emela_scheduler_run(1);
    emela_fiber_release(f);
    assert(result == (i % 2 ? -1 : 12));
    assert(!emela_task_complete_int(task, 0));
    assert(test_heap_empty());
  }
}
"#,
    );
}
