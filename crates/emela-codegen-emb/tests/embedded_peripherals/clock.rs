use super::*;

#[test]
fn clock_now_only_does_not_require_the_task_runtime() {
    // Direct Flow isolates the no-task bridge from the source handler's full operation table.
    let mut flow = Module::new(emela_core::Module::new());
    let mut binding = FnBuilder::new(&flow.core, "Std.Embedded.Board.clock_now");
    binding.set_ret(TypeId::INT64);
    let now = flow.add(binding.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("__emela_peripheral_clock_now".into()),
        }],
    }));
    let mut entry = FnBuilder::new(&flow.core, "current_time");
    entry.set_exported().set_ret(TypeId::INT64);
    let value = entry.assign(
        "now",
        TypeId::INT64,
        Value::Call {
            callee: Callee::Direct(now),
            args: vec![],
            suspends: false,
            on_fail: None,
        },
    );
    entry.term(Term::Return(Some(value.into())));
    flow.add(entry.finish());
    let output = compile_flow(&flow, &Options::default())
        .expect("emit valid Flow with only the standard clock_now binding");
    let header = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.h")
        .unwrap()
        .contents;
    assert!(!header.contains("#define EMELA_HAS_TASK_RUNTIME"));
    assert!(!header.contains("EmelaTask"));
    assert!(output.files.iter().any(|file| file.path == "emela_clock.h"));
    let current_time = export_name(&output, "current_time");
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_clock.h"
#include "emela_board.h"
#ifdef EMELA_HAS_TASK_RUNTIME
#error "Clock.now must not require the task runtime"
#endif

static unsigned reads;
int64_t emela_board_clock_now(void) {{
  ++reads;
  return INT64_C(1700000000123);
}}
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {{
  (void)kind; (void)message; (void)length;
  abort();
}}
int main(void) {{
  for (unsigned i = 0; i < 1000; ++i) {{
    assert({current_time}() == INT64_C(1700000000123));
  }}
  assert(reads == 1000);
  return 0;
}}
"#
        ),
    );
}

#[test]
fn clock_sleep_notifies_then_resumes_through_the_main_scheduler() {
    let output = source_output(
        r#"
import Std.Clock.{Clock}
import Std.Embedded.Board.{BoardClock}

pub fn wait_then_return(ms: Int) -> Int use Async {
  with BoardClock {
    (use Clock).sleep(ms)
    42
  }
}
"#,
    );
    let header = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.h")
        .unwrap()
        .contents;
    assert!(header.contains("#define EMELA_HAS_TASK_RUNTIME 1"));
    let wait_then_return = export_name(&output, "wait_then_return");
    assert!(wait_then_return.ends_with("_start"));
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_clock.h"
#include "emela_board.h"
#ifndef EMELA_HAS_TASK_RUNTIME
#error "Clock.sleep requires the task runtime"
#endif

static EmelaTask pending_task;
static int pending;
static int32_t requested_ms;
static unsigned starts;
static uint32_t locked;
int64_t emela_board_clock_now(void) {{ return INT64_C(0); }}
uint32_t emela_board_event_lock(void) {{
  uint32_t previous = locked;
  locked = 1;
  return previous;
}}
void emela_board_event_unlock(uint32_t previous) {{ locked = previous; }}
void emela_board_clock_sleep(EmelaTask task, int32_t milliseconds) {{
  assert(!locked && !pending);
  pending_task = task;
  requested_ms = milliseconds;
  pending = 1;
  ++starts;
}}
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {{
  (void)kind; (void)message; (void)length;
  abort();
}}
int main(void) {{
  static const int32_t delays[] = {{INT32_MIN, -1, 0, 7, INT32_MAX}};
  for (unsigned i = 0; i < 1000; ++i) {{
    int32_t milliseconds = delays[i % (sizeof delays / sizeof delays[0])];
    int32_t result = -1;
    EmelaFiber fiber = {wait_then_return}(milliseconds, &result);
    while (emela_scheduler_run(1)) {{}}
    assert(pending && starts == i + 1);
    assert(requested_ms == (milliseconds < 0 ? 0 : milliseconds));
    assert(result == -1 && !emela_fiber_done(fiber));
    pending = 0;
    assert(emela_task_complete_unit(pending_task));
    assert(!emela_task_complete_unit(pending_task));
    assert(result == -1 && !emela_fiber_done(fiber));
    while (!emela_fiber_done(fiber)) assert(emela_scheduler_run(1));
    assert(result == 42 && !locked);
    emela_fiber_release(fiber);
    assert(!emela_task_complete_unit(pending_task));
  }}
  return 0;
}}
"#
        ),
    );
}
