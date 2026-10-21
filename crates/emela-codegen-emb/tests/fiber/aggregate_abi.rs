use super::*;

fn runtime_fixture() -> Output {
    compile_runtime(
        include_str!("../../examples/support/fiber_aggregate_abi.emel"),
        1024,
    )
}

fn compile_runtime(source: &str, frame_bytes: u16) -> Output {
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    let mut output = compile(
        &core,
        &options()
            .with_heap_bytes(4096)
            .with_string_capacity(64)
            .with_frame_bytes(frame_bytes),
    )
    .unwrap();
    notes::add_heap_check(&mut output);
    output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents
        .push_str("void test_cancel(EmelaFiber h) { emela_fiber_get(h)->cancelled = 1; }\n");
    output
}

fn result_type(output: &Output, entry: &str) -> usize {
    output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.h")
        .unwrap()
        .contents
        .lines()
        .find(|line| line.starts_with(&format!("EmelaFiber emela_export_{entry}(")))
        .unwrap()
        .split(", emela_type_")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

fn run_runtime(output: Output, body: String) {
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
void test_cancel(EmelaFiber f);
static EmelaTask tasks[7];
static unsigned count;
uint32_t emela_board_event_lock(void) {{ return 0; }}
void emela_board_event_unlock(uint32_t p) {{ (void)p; }}
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ fprintf(stderr, "%d: %.*s\n", k,n,m); abort(); }}
void board_wait_start(EmelaTask t, int32_t n) {{ assert(n >= 10 && n <= 60 && n % 10 == 0); tasks[n / 10] = t; count++; }}
static void drain(void) {{ while (emela_scheduler_run(1)) {{}} }}
{body}
"#
        ),
    );
}

#[test]
fn both_returns_nested_string_aggregates_and_cancels_pending_children_without_leaks() {
    let output = runtime_fixture();
    let ty = result_type(&output, "both_0_start");
    let release = aggregate::release_helper(&output, ty);
    run_runtime(
        output,
        format!(
            r#"
int main(void) {{
  for (int i = 0; i < 1000; ++i) {{
    int cancel = i % 5, discard = i % 3 == 0;
    count = 0; emela_type_{ty} result = {{0}};
    EmelaFiber f = emela_export_both_0_start(42, discard ? NULL : &result);
    drain(); assert(count == 2 && !emela_fiber_done(f));
    if (cancel == 1) test_cancel(f);
    else {{
      unsigned first = i % 2 ? 1 : 2, second = 3 - first;
      assert(emela_task_complete_int(tasks[first], 1)); drain();
      assert(!emela_fiber_done(f));
      if (cancel == 2) test_cancel(f);
      else assert(emela_task_complete_int(tasks[second], 2));
    }}
    drain(); assert(emela_fiber_done(f)); emela_fiber_release(f);
    assert(!emela_task_complete_int(tasks[1], 0) && !emela_task_complete_int(tasks[2], 0));
    if (cancel != 1 && cancel != 2 && !discard) {{
      assert(result.f0.f0.owner && result.f0.f0.length == 2 && memcmp(result.f0.f0.data, "42", 2) == 0);
      assert(result.f0.f1.tag == 0 && result.f0.f1.payload.v0.f0.owner == result.f0.f0.owner);
      assert(result.f1.tag == 0 && result.f1.payload.v0.f0.owner && memcmp(result.f1.payload.v0.f0.data, "43", 2) == 0);
      {release}(&result);
    }}
    assert(test_heap_empty());
  }}
}}
"#
        ),
    );
}

#[test]
fn scope_returns_string_enum_exits_after_success_or_interruption_and_frame_reuse() {
    let output = runtime_fixture();
    let ty = result_type(&output, "terminal_1_start");
    let release = aggregate::release_helper(&output, ty);
    run_runtime(
        output,
        format!(
            r#"
int main(void) {{
  for (int i = 0; i < 1000; ++i) {{
    int cancel_child = i % 2, cancel_parent = i % 5 == 0, discard = i % 3 == 0;
    count = 0; emela_type_{ty} result = {{0}};
    EmelaFiber f = emela_export_terminal_1_start(42, cancel_child, discard ? NULL : &result);
    drain(); assert(count == 2 && !emela_fiber_done(f));
    if (cancel_parent) test_cancel(f);
    else {{
      if (!cancel_child && i % 4) {{ assert(emela_task_complete_int(tasks[3], 30)); drain(); }}
      assert(emela_task_complete_int(tasks[4], 40)); drain();
      if (!cancel_child && !emela_fiber_done(f)) assert(emela_task_complete_int(tasks[3], 30));
    }}
    drain(); assert(emela_fiber_done(f)); emela_fiber_release(f);
    if (!discard && !cancel_parent) {{
      if (cancel_child) assert(result.tag == 1 && result.payload.v1.f0);
      else {{
        assert(result.tag == 0 && result.payload.v0.f0.tag == 0);
        EmelaString text = result.payload.v0.f0.payload.v0.f0.f0;
        assert(text.owner && text.length == 2 && memcmp(text.data, "42", 2) == 0);
      }}
      {release}(&result);
    }}
    assert(test_heap_empty());
  }}
}}
"#
        ),
    );
}

#[test]
fn handler_resume_accepts_a_string_enum_and_reclaims_it_when_cancelled_in_either_wait() {
    let output = runtime_fixture();
    let ty = result_type(&output, "echoed_2_start");
    let release = aggregate::release_helper(&output, ty);
    run_runtime(
        output,
        format!(
            r#"
int main(void) {{
  for (int i = 0; i < 1000; ++i) {{
    int cancel = i % 5, discard = i % 3 == 0;
    count = 0; emela_type_{ty} result = {{0}};
    EmelaFiber f = emela_export_echoed_2_start(42, discard ? NULL : &result);
    drain(); assert(count == 1 && !emela_fiber_done(f));
    if (cancel == 1) test_cancel(f);
    else {{
      assert(emela_task_complete_int(tasks[5], 50)); drain();
      assert(count == 2 && !emela_fiber_done(f));
      if (cancel == 2) test_cancel(f);
      else assert(emela_task_complete_int(tasks[6], 60));
    }}
    drain(); assert(emela_fiber_done(f)); emela_fiber_release(f);
    if (cancel != 1 && cancel != 2 && !discard) {{
      assert(result.tag == 0 && result.payload.v0.f0.f0.owner);
      assert(result.payload.v0.f0.f0.length == 2 && memcmp(result.payload.v0.f0.f0.data, "42", 2) == 0);
      assert(result.payload.v0.f0.f1.tag == 0 && result.payload.v0.f0.f1.payload.v0.f0.owner && result.payload.v0.f0.f1.payload.v0.f0.length == 2 && memcmp(result.payload.v0.f0.f1.payload.v0.f0.data, "42", 2) == 0);
      {release}(&result);
    }}
    assert(test_heap_empty());
  }}
}}
"#
        ),
    );
}

#[test]
fn both_propagates_managed_failure_from_aggregate_callbacks_and_reclaims_its_sibling() {
    let source = [
        include_str!("../../examples/support/fiber_aggregate_abi.emel"),
        include_str!("../../examples/support/fiber_aggregate_failure.emel"),
    ]
    .concat();
    let output = compile_runtime(&source, 2048);
    run_runtime(
        output,
        r#"
int main(void) {
  for (int i = 0; i < 1000; ++i) {
    int bad = i % 2, cancel = i % 5 == 0, discard = i % 3 == 0;
    count = 0; EmelaString result = {0};
    EmelaFiber f = emela_export_checked_3_start(42, bad, discard ? NULL : &result);
    drain(); assert(count == 2 && !emela_fiber_done(f));
    if (cancel) test_cancel(f);
    else {
      assert(emela_task_complete_int(tasks[2], 20)); drain();
      if (!bad) { assert(!emela_fiber_done(f)); assert(emela_task_complete_int(tasks[1], 10)); }
    }
    drain(); assert(emela_fiber_done(f)); emela_fiber_release(f);
    if (!cancel && !discard) {
      assert(result.owner && result.length == (bad ? 2 : 4));
      assert(memcmp(result.data, bad ? "42" : "4343", result.length) == 0);
      emela_embedded_release_string(&result);
    }
    assert(test_heap_empty());
  }
}
"#
        .to_owned(),
    );
}
