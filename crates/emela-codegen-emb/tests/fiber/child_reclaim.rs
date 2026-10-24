use super::*;

fn fixture(heap_bytes: u32) -> Output {
    let core = support::try_lower(
        &[(
            "main.emel",
            include_str!("../../examples/support/fiber_child_reclaim.emel"),
        )],
        Roots::Exported,
    )
    .unwrap();
    let mut output = compile(
        &core,
        &options()
            .with_heap_bytes(heap_bytes)
            .with_string_capacity(32)
            .with_fiber_capacity(4)
            .with_task_capacity(1)
            .with_event_capacity(1),
    )
    .unwrap();
    instrument(&mut output);
    output
}

fn instrument(output: &mut Output) {
    notes::add_heap_check(output);
    output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents
        .push_str(
            r#"
int test_children(void) {
  int children = 0;
  if (!emela_heap_initialized) return 0;
  for (unsigned char *at = emela_heap_storage; at < emela_heap_end();) {
    emela_heap_block *block = (emela_heap_block *)(void *)at;
    if (!block->is_free && block->kind == EMELA_CHILD_KIND) children++;
    at += block->size;
  }
  return children;
}
void test_cancel(EmelaFiber h) { emela_fiber_get(h)->cancelled = 1; }
static emela_child *test_child(emela_scope *scope) {
  emela_child *child = emela_heap_allocate(sizeof(*child), EMELA_CHILD_KIND);
  *child = (emela_child){.scope = scope, .next = scope->children, .status = 1};
  if (scope->children != NULL) scope->children->previous = child;
  scope->children = child;
  emela_heap_retain(scope);
  return child;
}
void test_weak_child_links(void) {
  emela_scope *scope = emela_heap_allocate(sizeof(*scope), EMELA_SCOPE_KIND);
  *scope = (emela_scope){0};
  emela_child *tail = test_child(scope), *middle = test_child(scope), *head = test_child(scope);
  emela_heap_release(middle);
  if (scope->children != head || head->next != tail || tail->previous != head) emela_scheduler_invalid();
  emela_heap_release(head);
  if (scope->children != tail || tail->previous != NULL) emela_scheduler_invalid();
  emela_heap_release(tail);
  if (scope->children != NULL) emela_scheduler_invalid();
  tail = test_child(scope); head = test_child(scope);
  head->result.payload.any_box = tail;
  emela_heap_retain(tail);
  emela_heap_release(tail);
  scope->closed = 1;
  emela_scope_finish(scope);
  if (scope->children != NULL || head->result.payload.any_box != NULL) emela_scheduler_invalid();
  emela_heap_release(head);
  emela_heap_release(scope);
  scope = emela_heap_allocate(sizeof(*scope), EMELA_SCOPE_KIND);
  *scope = (emela_scope){0};
  tail = test_child(scope); head = test_child(scope);
  head->result.payload.any_box = tail; emela_heap_retain(tail);
  tail->result.payload.any_box = head; emela_heap_retain(head);
  emela_heap_release(tail); emela_heap_release(head);
  scope->closed = 1;
  emela_scope_finish(scope);
  if (scope->children != NULL) emela_scheduler_invalid();
  emela_heap_release(scope);
}
"#,
        );
}

#[test]
fn long_lived_scope_reclaims_completed_children_with_copied_handles_and_repeated_join_await() {
    compile_and_run(
        fixture(2048),
        r#"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_children(void);
int test_heap_empty(void);
void test_weak_child_links(void);
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t s) { (void)s; }
void board_wait_start(EmelaTask t, int32_t n) { (void)t; (void)n; abort(); }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; ++i) {
    test_weak_child_links(); assert(test_heap_empty());
    int32_t result = -1;
    EmelaFiber f = emela_export_work_0_start(1000, i % 2 ? &result : NULL);
    while (!emela_fiber_done(f)) {
      assert(emela_scheduler_run(1));
      assert(test_children() <= 2);
    }
    if (i % 2) assert(result == 3000);
    emela_fiber_release(f);
    assert(test_children() == 0 && test_heap_empty());
  }
}
"#,
    );
}

#[test]
fn discarded_completed_children_release_results_before_scope_end_and_survive_cancellation() {
    compile_and_run(
        fixture(2048),
        r#"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_children(void);
int test_heap_empty(void);
void test_cancel(EmelaFiber);
static EmelaTask task;
static int started;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t s) { (void)s; }
void board_wait_start(EmelaTask t, int32_t n) { assert(n == ++started); task = t; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; ++i) {
    int32_t result = -1; started = 0;
    EmelaFiber f = emela_export_abandoned_1_start(42, &result);
    while (emela_scheduler_run(1)) {}
    assert(started == 1 && !emela_fiber_done(f) && test_children() == 0);
    assert(emela_task_complete_int(task, 0));
    while (emela_scheduler_run(1)) {}
    assert(started == 2 && !emela_fiber_done(f) && test_children() == 0);
    if (i % 2) test_cancel(f);
    else assert(emela_task_complete_int(task, 0));
    while (!emela_fiber_done(f)) {
      uint32_t steps = emela_scheduler_run(1);
      assert(steps || emela_fiber_done(f));
    }
    assert(result == (i % 2 ? -1 : 42));
    assert(!emela_task_complete_int(task, 0));
    emela_fiber_release(f);
    assert(test_heap_empty());
  }
}
"#,
    );
}

#[test]
fn live_handles_keep_completed_results_for_repeated_join_await_after_execution_slot_reuse() {
    compile_and_run(
        fixture(4096),
        r#"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_children(void);
int test_heap_empty(void);
static EmelaTask task;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t s) { (void)s; }
void board_wait_start(EmelaTask t, int32_t n) { assert(n == 1); task = t; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; ++i) {
    void *result = NULL;
    EmelaFiber f = emela_export_kept_2_start(42, i % 3 ? &result : NULL);
    while (emela_scheduler_run(1)) {}
    assert(!emela_fiber_done(f) && test_children() == 1);
    for (int n = 0; n < 10; ++n) {
      EmelaFiber g = emela_export_work_0_start(1, NULL);
      while (!emela_fiber_done(g)) assert(emela_scheduler_run(1));
      emela_fiber_release(g);
      assert(test_children() == 1);
    }
    assert(emela_task_complete_int(task, 0));
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
    assert(test_children() == 0);
    if (i % 3) {
      for (int n = 0; n < 3; ++n) {
        EmelaString text = emela_export_head_3(result);
        assert(text.owner && text.length == 2 && memcmp(text.data, "42", 2) == 0);
        emela_embedded_release_string(&text);
        void *next = emela_export_tail_4(result);
        emela_embedded_release(result); result = next;
      }
      assert(result == NULL);
    }
    assert(test_heap_empty());
  }
}

"#,
    );
}

#[test]
fn failed_handleless_children_release_records_while_scope_waits_for_sibling_cleanup() {
    let source = include_str!("../../examples/support/fiber_child_failure_reclaim.emel");
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    let mut output = compile(
        &core,
        &options()
            .with_heap_bytes(4096)
            .with_string_capacity(64)
            .with_frame_capacity(40)
            .with_frame_bytes(768)
            .with_task_capacity(3)
            .with_event_capacity(3),
    )
    .unwrap();
    instrument(&mut output);
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_children(void);
int test_heap_empty(void);
void test_cancel(EmelaFiber);
static EmelaTask tasks[3];
static int started;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t s) { (void)s; }
void board_wait_start(EmelaTask t, int32_t n) { assert(n >= 1 && n <= 3); tasks[n - 1] = t; ++started; }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; ++i) {
    EmelaString result = {0}; started = 0;
    EmelaFiber f = emela_export_work_0_start(42, i % 3 ? &result : NULL);
    while (emela_scheduler_run(1)) {}
    assert(started == 3 && !emela_fiber_done(f));
    assert(test_children() == 1);
    if (i % 2) { test_cancel(f); while (emela_scheduler_run(1)) {} }
    assert(!emela_fiber_done(f) && test_children() == 1);
    assert(emela_task_complete_int(tasks[2], 0));
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
    assert(!emela_task_complete_int(tasks[0], 0) && !emela_task_complete_int(tasks[1], 0));
    assert(test_children() == 0);
    if (i % 3 && i % 2 == 0) {
      assert(result.owner && result.length == 2 && memcmp(result.data, "42", 2) == 0);
      emela_embedded_release_string(&result);
    } else assert(result.data == NULL && result.owner == NULL && result.length == 0);
    assert(test_heap_empty());
  }
}
"#,
    );
}
