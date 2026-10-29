use super::*;
use crate::gc_anchor;

#[path = "../../examples/support/phase7_integration_flow.rs"]
mod fixture;

#[test]
fn shared_state_child_reclamation_deferreds_and_external_failure_share_owned_edges_with_and_without_gc()
 {
    run(false);
}

#[test]
fn managed_task_success_values_share_state_child_and_deferred_ownership_with_and_without_gc() {
    run(true);
}

fn run(managed_success: bool) {
    for with_collector in [false, true] {
        let flow = if managed_success {
            fixture::lower_managed(with_collector)
        } else {
            fixture::lower(with_collector)
        };
        let mut output = compile_flow(
            &flow,
            &options()
                .with_heap_bytes(4096)
                .with_string_capacity(32)
                .with_frame_capacity(48)
                .with_frame_bytes(2048)
                .with_deferred_capacity(2)
                .with_task_adapter("host_request", "board_request_start"),
        )
        .unwrap();
        let header = &output
            .files
            .iter()
            .find(|file| file.path == "emela_embedded.h")
            .unwrap()
            .contents;
        let error = header
            .lines()
            .find(|line| {
                line.trim_start().starts_with("emela_type_")
                    && line.trim_end().ends_with(';')
                    && line.contains(" e")
            })
            .unwrap();
        let mut fields = error.split_whitespace();
        let ty = fields.next().unwrap().trim_start_matches("emela_type_");
        let tag: usize = fields
            .next()
            .unwrap()
            .trim_start_matches('e')
            .trim_end_matches(';')
            .parse()
            .unwrap();
        let anchor = if with_collector {
            let function = header
                .lines()
                .find(|line| line.contains("emela_export_gc_anchor_"))
                .unwrap()
                .split('(')
                .next()
                .unwrap()
                .split_whitespace()
                .last()
                .unwrap();
            format!(
                "EmelaAny anchored = {function}((EmelaAny){{0}}); assert(anchored.tag == 0 && anchored.payload.any_box == NULL);"
            )
        } else {
            String::new()
        };
        let success_helpers = if managed_success {
            r#"
static int success_produced, success_released;
static void produce_success(void *p, EmelaString *value) {
  Context *context = p; assert(!isr && !locked); ++success_produced;
  *value = (EmelaString){.data = context->message, .length = 2};
}
static void release_success(void *p) {
  Context *context = p; assert(!isr && !locked); ++success_released;
  memset(context->message, 'x', 3);
}
"#
        } else {
            ""
        };
        let success = if managed_success {
            format!(
                r#"
      Context context = {{.message = {{'4', '2', '!'}}}};
      int produced_before = success_produced, released_before = success_released;
      {anchor}
      isr = 1; assert(emela_task_complete_value_{}(task, produce_success, &context, release_success)); isr = 0;
      assert(success_produced == produced_before && success_released == released_before);
      drain();
      assert(success_produced == produced_before + 1 && success_released == released_before + 1);
      assert(memcmp(context.message, "xxx", 3) == 0);
"#,
                TypeId::STRING.index(),
            )
        } else {
            "assert(emela_task_complete_int(task, 42)); drain();".to_owned()
        };
        let reset_success = if managed_success {
            "success_produced = success_released = 0;"
        } else {
            ""
        };
        let cancel_success = if managed_success {
            format!(
                r#"
      if (mode == 3) {{
        Context context = {{.message = {{'4', '2', '!'}}}};
        {anchor}
        isr = 1; assert(emela_task_complete_value_{}(task, produce_success, &context, release_success)); isr = 0;
        test_cancel(f); (void)emela_scheduler_run(0);
        assert(!success_produced && !success_released);
        drain();
        assert(emela_fiber_done(f) && !success_produced && success_released == 1 && !produced && !released);
        assert(memcmp(context.message, "xxx", 3) == 0);
        break;
      }}
"#,
                TypeId::STRING.index(),
            )
        } else {
            String::new()
        };
        let check_success = if managed_success {
            "assert(success_produced == (mode == 0 ? count : mode == 2 ? count - 1 : 0) && success_released == success_produced + (mode == 3));"
        } else {
            ""
        };
        let harness = format!(
            r#"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
int test_children(void);
void test_cancel(EmelaFiber);
static EmelaTask task;
static int requests, expected_request, produced, released, locked, isr;
typedef struct {{ char message[3]; void *parts; }} Context;
{success_helpers}
uint32_t emela_board_event_lock(void) {{ assert(!locked); locked = 1; return 0; }}
void emela_board_event_unlock(uint32_t s) {{ (void)s; assert(locked); locked = 0; }}
void board_request_start(EmelaTask t, int32_t n) {{ assert(n == expected_request--); task = t; ++requests; }}
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }}
static void produce(void *p, EmelaError *e) {{
  Context *context = p; assert(!isr && !locked); ++produced;
  *e = (EmelaError){{.tag = {error_tag}, .payload.e{tag} = (emela_type_{ty}){{.tag = 0, .payload.v0 = {{
    .f0 = {{.data = context->message, .length = 3}}, .f1 = context->parts
  }}}}}};
}}
static void release_context(void *p) {{
  Context *context = p; assert(!isr && !locked); ++released;
  memset(context->message, 'x', 3); emela_embedded_release(context->parts); context->parts = NULL;
}}
static void drain(void) {{ while (emela_scheduler_run(1)) {{ assert(test_children() <= 3); }} }}
int main(void) {{
  for (int i = 0; i < 1000; ++i) {{
    const int mode = i % 4, count = i == 0 ? 1000 : 3, discarded = i % 5 == 4;
    int32_t result = -1; requests = produced = released = 0; expected_request = count;
    {reset_success}
    EmelaFiber f = emela_export_work_0_start(count, discarded ? NULL : &result);
    drain();
    for (int n = 0; n < count; ++n) {{
      assert(!emela_fiber_done(f) && requests == n + 1 && test_children() <= 2);
      {cancel_success}
      if (((mode == 1 || mode == 3) && n == 0) || (mode == 2 && n == count - 1)) {{
        Context context = {{.message = {{'a', '\0', 'b'}}, .parts = emela_export_parts_1(42)}};
        {anchor}
        isr = 1; assert(emela_task_fail(task, produce, &context, release_context)); isr = 0;
        assert(!produced && !released);
        if (mode == 3) {{ test_cancel(f); (void)emela_scheduler_run(0); assert(!produced && !released); }}
        drain();
        assert(emela_fiber_done(f) && produced == (mode != 3) && released == 1 && context.parts == NULL);
        break;
      }}
      {success}
    }}
    assert(emela_fiber_done(f)); emela_fiber_release(f);
    if (!discarded && mode != 3) assert(result == (mode == 0 ? 2 * count + 2 : mode == 1 ? 11 : 6));
    else assert(result == -1);
    assert(!emela_task_complete_int(task, 0));
    {check_success}
    assert(test_children() == 0 && test_heap_empty());
  }}
}}
"#,
            error_tag = tag + 1,
        );
        notes::add_heap_check(&mut output);
        let source = &mut output
            .files
            .iter_mut()
            .find(|file| file.path == "emela_embedded.c")
            .unwrap()
            .contents;
        assert_eq!(
            source.contains("static void emela_heap_collect_cycles("),
            with_collector
        );
        source.push_str(
            r#"
void test_cancel(EmelaFiber h) { emela_fiber_get(h)->cancelled = 1; }
int test_children(void) {
  int result = 0;
  for (unsigned char *p = emela_heap_storage; emela_heap_initialized && p < emela_heap_end();) {
    emela_heap_block *block = (emela_heap_block *)(void *)p;
    if (!block->is_free && block->kind == EMELA_CHILD_KIND) ++result;
    p += block->size;
  }
  return result;
}
"#,
        );
        compile_and_run(output, &harness);
    }
}
