//! The embedded C target through `emela build`: the generated C is checked as text, compiled with
//! the host cc against a stub board that logs the calls it gets, and run. Tests that need cc are
//! skipped, with a message, when it cannot start.

use std::path::{Path, PathBuf};
use std::process::Command;

use emela_driver::{
    Analysis, CoreC, EmbeddedC, Input, Lower, LowerTests, OsFs, Output, Target, TestOptions,
    TestOutcome, build, cc_command, test,
};

/// The stub board: logs every GPIO call and defect to stdout. After `main`, the first call of
/// `emela_board_idle` runs the interrupt entry with `STUB_EVENTS` (if the program has one), then
/// the program exits.
const STUB_BOARD: &str = r#"#include <stdio.h>
#include <stdlib.h>
#include "emela_embedded.h"

#ifdef EMELA_GPIO_H
void emela_gpio_configure(EmelaGpioPin pin, EmelaGpioMode mode, EmelaGpioPull pull) {
  printf("configure %d %d %d\n", (int)pin, (int)mode, (int)pull);
}
EmelaGpioPinState emela_gpio_read(EmelaGpioPin pin) {
  printf("read %d\n", (int)pin);
  return EMELA_GPIO_PIN_STATE_HIGH;
}
void emela_gpio_write(EmelaGpioPin pin, EmelaGpioPinState state) {
  printf("write %d %d\n", (int)pin, (int)state);
}
void emela_gpio_toggle(EmelaGpioPin pin) { printf("toggle %d\n", (int)pin); }
void emela_gpio_enable_interrupt(EmelaGpioPin pin, EmelaGpioEdge edge) {
  printf("enable_interrupt %d %d\n", (int)pin, (int)edge);
}
void emela_gpio_disable_interrupt(EmelaGpioPin pin) { printf("disable_interrupt %d\n", (int)pin); }
#endif

void emela_board_init(void) {}

_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
  printf("defect %d %.*s\n", (int)kind, (int)length, message);
  exit(3);
}

void emela_board_console_write(const char *bytes, uint16_t length) {
  fwrite(bytes, 1, length, stdout);
}
void emela_board_console_error_write(const char *bytes, uint16_t length) {
  printf("err:");
  fwrite(bytes, 1, length, stdout);
}
int64_t emela_board_now_milliseconds(void) { return 1000; }
void emela_board_sleep_milliseconds(int32_t ms) { printf("sleep %d\n", (int)ms); }
int32_t emela_board_random_int(void) { return 7; }
double emela_board_random_float(void) { return 0.5; }

void emela_board_debug(const char *bytes, uint16_t length) {
  printf("dbg %.*s\n", (int)length, bytes);
}

static int critical_depth;
void emela_board_critical_enter(void) { critical_depth++; }
void emela_board_critical_exit(void) {
  if (--critical_depth < 0) { printf("unbalanced critical section\n"); exit(4); }
}

void emela_board_idle(void) {
#ifdef STUB_EVENTS
  STUB_EVENTS
#endif
  printf("idle\n");
  exit(critical_depth == 0 ? 0 : 5);
}
"#;

/// Whether the C compiler starts; prints why a test is skipped otherwise.
fn have_cc() -> bool {
    let (cc, flags) = cc_command();
    let ok = Command::new(&cc)
        .args(&flags)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("skipping: cannot start the C compiler `{}`", cc.display());
    }
    ok
}

/// A per-test directory on disk (the project in `app/`, the output in `out/`). Leftovers from
/// earlier runs are removed.
fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "emela-driver-c-embedded-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Writes the project (`main.emel` and other files of `src/`) under `dir` and builds it for the
/// embedded C target into `dir/out`.
fn build_embedded(dir: &Path, files: &[(&str, &str)]) -> (Analysis, Option<PathBuf>) {
    let src = dir.join("app/src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        dir.join("app/Pome.toml"),
        "[build]\ntarget = \"c-embedded\"\n",
    )
    .unwrap();
    // A `Pome.toml` among the files replaces the default one.
    for (name, contents) in files {
        if *name == "Pome.toml" {
            std::fs::write(dir.join("app/Pome.toml"), contents).unwrap();
        } else {
            std::fs::write(src.join(name), contents).unwrap();
        }
    }
    let input = Input::resolve(&OsFs, &dir.join("app")).unwrap();
    build(
        &OsFs,
        &input,
        &mut Lower::default().with_target(Target::CEmbedded),
        &mut EmbeddedC {
            options: emela_driver::embedded_options(&OsFs, &input).unwrap(),
        },
        &dir.join("out"),
    )
}

/// The error messages of a build that must fail.
fn errors(main: &str, name: &str) -> Vec<String> {
    let dir = test_dir(name);
    let (analysis, entry) = build_embedded(&dir, &[("main.emel", main)]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(entry.is_none(), "the build should fail");
    analysis
        .diagnostics
        .iter()
        .map(|d| d.message.clone())
        .collect()
}

/// Builds `main`, compiles it with the stub board (with `-D STUB_EVENTS=…` if given) and runs it;
/// the stdout and the exit code.
fn run(main: &str, name: &str, events: Option<&str>) -> Option<(String, i32)> {
    run_project(&[("main.emel", main)], "", name, events)
}

/// [`run`] for a project with more files, with `board` appended to the stub board. Every C file
/// of the output is compiled.
fn run_project(
    files: &[(&str, &str)],
    board: &str,
    name: &str,
    events: Option<&str>,
) -> Option<(String, i32)> {
    if !have_cc() {
        return None;
    }
    let dir = test_dir(name);
    let (analysis, entry) = build_embedded(&dir, files);
    assert!(entry.is_some(), "{:#?}", analysis.diagnostics);
    let out = dir.join("out");
    std::fs::write(out.join("board.c"), format!("{STUB_BOARD}{board}")).unwrap();
    let mut sources: Vec<PathBuf> = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "c"))
        .collect();
    sources.sort();
    let (cc, flags) = cc_command();
    let exe = out.join("firmware");
    let mut cmd = Command::new(&cc);
    cmd.args(&flags)
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-O1"])
        .arg("-I")
        .arg(&out)
        .args(&sources)
        .arg("-lm")
        .arg("-o")
        .arg(&exe);
    if let Some(events) = events {
        cmd.arg(format!("-DSTUB_EVENTS={events}"));
    }
    let compiled = cmd.output().unwrap();
    assert!(
        compiled.status.success(),
        "cc failed:\n{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let ran = Command::new(&exe).output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    Some((
        String::from_utf8(ran.stdout).unwrap(),
        ran.status.code().unwrap_or(-1),
    ))
}

const GPIO_EXAMPLE: &str = include_str!("../../../examples/embedded-gpio/src/main.emel");

/// The interrupt events the stub sends to the example: the button pressed, then released.
const BUTTON_EVENTS: &str = "emela_export_button_changed((EmelaGpioEvent){.pin = 100, .level = \
                             EMELA_GPIO_PIN_STATE_LOW}); emela_export_button_changed((\
                             EmelaGpioEvent){.pin = 100, .level = EMELA_GPIO_PIN_STATE_HIGH}); \
                             emela_export_button_changed((EmelaGpioEvent){.pin = 5, .level = \
                             EMELA_GPIO_PIN_STATE_LOW});";

#[test]
fn gpio_example_output() {
    let dir = test_dir("example-output");
    let (analysis, entry) = build_embedded(&dir, &[("main.emel", GPIO_EXAMPLE)]);
    assert!(entry.is_some(), "{:#?}", analysis.diagnostics);
    let out = dir.join("out");
    let read = |f: &str| std::fs::read_to_string(out.join(f)).unwrap();
    let all = format!(
        "// main.c\n{}\n// emela_gpio.h\n{}\n// emela_embedded.h\n{}",
        read("main.c"),
        read("emela_gpio.h"),
        read("emela_embedded.h")
    );
    let _ = std::fs::remove_dir_all(&dir);
    insta::assert_snapshot!(all);
}

#[test]
fn gpio_example_runs_on_a_stub_board() {
    let Some((stdout, code)) = run(GPIO_EXAMPLE, "example-run", Some(BUTTON_EVENTS)) else {
        return;
    };
    assert_eq!(code, 0);
    assert_eq!(
        stdout,
        "configure 13 1 0\nwrite 13 0\nconfigure 100 0 1\nenable_interrupt 100 2\n\
         write 13 1\nwrite 13 0\nidle\n"
    );
}

#[test]
fn value_types_and_loops() {
    let main = r#"import Std.Embedded.Gpio.{Gpio, GpioPin, High, Low, PinState}

type Blink(pin: GpioPin, times: Int)
enum Step { On(GpioPin), Off(GpioPin), Wait(Int) }

fn step(n: Int, b: Blink) -> Step {
  if n % 2 == 0 { On(b.pin) } else { Off(b.pin) }
}

fn run_steps(b: Blink, n: Int) -> (Int, Int) use { Gpio } {
  gpio = use Gpio
  if n >= b.times * 2 {
    (n, n / 2)
  } else {
    step(n, b) match {
      On(p) -> gpio.write(p, High)
      Off(p) -> gpio.write(p, Low)
      Wait(_) -> ()
    }
    run_steps(b, n + 1)
  }
}

fn main() {
  (steps, blinks) = run_steps(Blink(pin: GpioPin(7), times: 2), 0)
  dbg(steps * 10 + blinks)
  ()
}
"#;
    let Some((stdout, code)) = run(main, "value-types", None) else {
        return;
    };
    assert_eq!(code, 0);
    assert_eq!(
        stdout,
        "write 7 1\nwrite 7 0\nwrite 7 1\nwrite 7 0\ndbg 42\nidle\n"
    );
}

#[test]
fn defects_stop_on_the_board() {
    let main = r#"import Std.Embedded.Gpio.{Gpio, GpioPin, High}

fn divide(a: Int, b: Int) -> Int { a / b }

fn main() {
  dbg(divide(10, 5))
  dbg(divide(1, 0))
  ()
}
"#;
    let Some((stdout, code)) = run(main, "defect", None) else {
        return;
    };
    assert_eq!(code, 3);
    assert_eq!(stdout, "dbg 2\ndefect 2 division by zero\n");
}

#[test]
fn unsupported_features_are_reported() {
    let heap = errors(
        "fn main() {\n  xs = [1, 2]\n  dbg(List.length(xs))\n}\n",
        "heap",
    );
    assert!(
        heap.iter().any(|m| m.contains("a value on the heap")),
        "{heap:#?}"
    );
    let stored = errors(
        "fn keep(f: fn(Int) -> Int) -> (fn(Int) -> Int, Int) { (f, 1) }\n\
         fn inc(x: Int) -> Int { x + 1 }\n\
         fn main() {\n  (g, n) = keep(inc)\n  dbg(g(n))\n}\n",
        "stored-closure",
    );
    assert!(
        stored.iter().any(|m| m.contains("function value")),
        "{stored:#?}"
    );
}

#[test]
fn recursion_from_an_interrupt_is_rejected() {
    let main = r#"import Std.Embedded.Gpio.{Gpio, GpioEvent, GpioPin, High}

fn count(n: Int) -> Int { if n <= 0 { 0 } else { 1 + count(n - 1) } }

pub fn changed(event: GpioEvent) use { Gpio } {
  (use Gpio).write(GpioPin(count(event.pin.id)), High)
}

fn main() { () }
"#;
    let messages = errors(main, "isr-recursion");
    assert!(
        messages
            .iter()
            .any(|m| m.contains("`count`, reached from an interrupt entry, is recursive")),
        "{messages:#?}"
    );
}

#[test]
fn external_functions_and_host_handles() {
    let main = r#"import Std.Embedded.Gpio.{Gpio, GpioPin, High, Low}

@external
type Sensor

@external(embedded, "sensor_open")
fn sensor_open(channel: Int) -> Sensor use {}

@external(embedded, "./sensor.c", "sensor_read")
fn sensor_read(sensor: Sensor) -> Float use {}

type Probe(sensor: Sensor, pin: GpioPin)

fn hot(p: Probe) -> Bool { sensor_read(p.sensor) > 20.0 }

fn main() {
  p = Probe(sensor: sensor_open(3), pin: GpioPin(9))
  gpio = use Gpio
  gpio.write(p.pin, if hot(p) { High } else { Low })
  dbg(Float.round(sensor_read(p.sensor)))
  ()
}
"#;
    let sensor = r#"#include "emela_embedded.h"
double sensor_read(void *sensor) { return *(const double *)sensor; }
"#;
    let board = r#"
static double channels[4] = {0.0, 0.0, 0.0, 21.5};
void *sensor_open(int32_t channel) { printf("open %d\n", (int)channel); return &channels[channel]; }
"#;
    let Some((stdout, code)) = run_project(
        &[("main.emel", main), ("sensor.c", sensor)],
        board,
        "external",
        None,
    ) else {
        return;
    };
    assert_eq!(code, 0);
    assert_eq!(stdout, "open 3\nwrite 9 1\ndbg 22\nidle\n");
}

#[test]
fn external_functions_need_an_embedded_binding() {
    let messages = errors(
        "@external(c, \"host_only\")\nfn host_only() -> Int use {}\n\n\
         fn main() {\n  dbg(host_only())\n}\n",
        "external-binding",
    );
    assert!(
        messages.iter().any(|m| m.contains("embedded")),
        "{messages:#?}"
    );
}

#[test]
fn user_handlers_are_static() {
    let main = r#"import Std.Embedded.Gpio.{Gpio, GpioPin, High, Low, PinState}

effect Led {
  fn set(on: Bool) -> ()
}

handler BoardLed(pin: GpioPin) implements Led {
  init { BoardLed(pin: GpioPin(13)) }
  fn set(on) {
    (use Gpio).write(self.pin, if on { High } else { Low })
  }
}

## A GPIO that logs writes as toggles of pin 99 instead.
handler Mirror implements Gpio {
  fn configure(pin, mode, pull) { () }
  fn read(pin) { Low }
  fn write(pin, state) { (use Gpio).toggle(GpioPin(99)) }
  fn toggle(pin) { () }
  fn enable_interrupt(pin, edge) { () }
  fn disable_interrupt(pin) { () }
}

fn blink(n: Int) use { Led } {
  if n > 0 {
    led = use Led
    led.set(True)
    led.set(False)
    blink(n - 1)
  }
}

fn shifted(o: Option[Int], by: Int) -> Option[Int] { Option.map(o, fn(x) { x + by }) }

fn main() {
  with BoardLed {
    blink(2)
  }
  with Mirror {
    with BoardLed {
      blink(1)
    }
  }
  dbg(shifted(Some(1), 4))
  ()
}
"#;
    let Some((stdout, code)) = run(main, "handlers", None) else {
        return;
    };
    assert_eq!(code, 0);
    assert_eq!(
        stdout,
        "write 13 1\nwrite 13 0\nwrite 13 1\nwrite 13 0\ntoggle 99\ntoggle 99\ndbg Some(5)\nidle\n"
    );
}

#[test]
fn state_run_keeps_its_state_on_the_stack() {
    let main = r#"import Std.Embedded.Gpio.{Gpio, GpioPin, High}
import Std.State.{State}

type Counter(n: Int, last: Int)

fn tick(by: Int) use { State[Counter] } {
  st = use State[Counter]
  st.update(fn(c) { Counter(n: c.n + by, last: by) })
}

fn main() {
  (_, c) = State.run(Counter(n: 1, last: 0), fn() {
    tick(2)
    tick(3)
  })
  dbg(c.n * 10 + c.last)
  ()
}
"#;
    let Some((stdout, code)) = run(main, "state", None) else {
        return;
    };
    assert_eq!(code, 0);
    assert_eq!(stdout, "dbg 63\nidle\n");
}

const PAUSE: &str = r#"import Std.Embedded.Gpio.{Gpio, GpioEvent, GpioPin, High, Low}

effect Pause {
  suspend fn wait(ms: Int) -> Int
}

@external(embedded, "board_wait")
suspend fn board_wait(ms: Int) -> ()

handler BoardPause implements Pause {
  suspend fn wait(ms, resume) {
    board_wait(ms)
    resume(ms / 10)
  }
}

fn blink() use { Pause, Gpio } {
  gpio = use Gpio
  gpio.write(GpioPin(5), High)
  n = (use Pause).wait(100)
  dbg(n)
  gpio.write(GpioPin(5), Low)
}
"#;

#[test]
fn suspending_operations_wait_until_done() {
    let main = format!("{PAUSE}\nfn main() {{\n  with BoardPause {{ blink() }}\n}}\n");
    let board = "\nvoid board_wait(int32_t ms) { printf(\"wait %d\\n\", (int)ms); }\n";
    let Some((stdout, code)) = run_project(&[("main.emel", &main)], board, "suspend", None) else {
        return;
    };
    assert_eq!(code, 0);
    assert_eq!(stdout, "write 5 1\nwait 100\ndbg 10\nwrite 5 0\nidle\n");
}

#[test]
fn waiting_in_an_interrupt_is_rejected() {
    let main = format!(
        "{PAUSE}\npub fn changed(event: GpioEvent) use {{ Gpio, Async }} {{\n  with BoardPause {{ blink() }}\n}}\n\nfn main() {{ () }}\n"
    );
    let messages = errors(&main, "suspend-isr");
    assert!(
        messages
            .iter()
            .any(|m| m.contains("board_wait") && m.contains("interrupt")),
        "{messages:#?}"
    );
}

#[test]
fn resuming_later_is_rejected() {
    let main = r#"effect Pause {
  suspend fn wait(ms: Int) -> ()
}

handler Late implements Pause {
  suspend fn wait(ms, resume) {
    resume(())
    dbg(ms)
    ()
  }
}

fn main() {
  with Late { (use Pause).wait(1) }
}
"#;
    let messages = errors(main, "resume-later");
    assert!(
        messages.iter().any(|m| m.contains("resume")),
        "{messages:#?}"
    );
}

#[test]
fn consts_are_evaluated_at_compile_time() {
    let main = r#"import Std.Embedded.Gpio.{Gpio, GpioPin, High, Low}

type Config(led: GpioPin, times: Int)

const LED = GpioPin(13)
const CONFIG = Config(led: LED, times: 2 * 3 + 1)
const LIMIT = fib(10)
const UNUSED = 1 / zero()
const BROKEN = 10 / zero()

fn zero() -> Int { 0 }
fn fib(n: Int) -> Int { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } }

fn main() {
  gpio = use Gpio
  gpio.write(CONFIG.led, High)
  dbg(CONFIG.times + LIMIT)
  dbg(BROKEN)
  ()
}
"#;
    let dir = test_dir("consts-output");
    let (analysis, entry) = build_embedded(&dir, &[("main.emel", main)]);
    assert!(entry.is_some(), "{:#?}", analysis.diagnostics);
    let c = std::fs::read_to_string(dir.join("out/main.c")).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(c.contains("_value = {.led = 13, .times = 7};"), "{c}");
    assert!(c.contains("_value = 55;"), "{c}");
    assert!(!c.contains("fib"), "{c}");
    let Some((stdout, code)) = run(main, "consts", None) else {
        return;
    };
    assert_eq!(code, 3);
    assert_eq!(stdout, "write 13 1\ndbg 62\ndefect 2 division by zero\n");
}

#[test]
fn static_states_outlive_interrupts() {
    let main = r#"import Std.Embedded.Gpio.{Gpio, GpioPin, GpioEvent, High, OutputPushPull, NoPull}
import Std.State.{State}

type Presses(n: Int)

@static
const PRESSES = Presses(n: 0)

pub fn button_changed(event: GpioEvent) use { Gpio, State[Presses] } {
  gpio = use Gpio
  st = use State[Presses]
  st.update(fn(p) { Presses(n: p.n + 1) })
  if st.get().n >= 3 { gpio.toggle(GpioPin(13)) } else { () }
}

fn main() {
  (use Gpio).configure(GpioPin(13), OutputPushPull, NoPull)
  (use State[Presses]).set(Presses(n: 1))
}
"#;
    let press = "emela_export_button_changed((EmelaGpioEvent){.pin = 2, .level = EMELA_GPIO_PIN_STATE_LOW});";
    let events = press.repeat(3);
    let Some((stdout, code)) = run(main, "static-state", Some(&events)) else {
        return;
    };
    assert_eq!(code, 0);
    assert_eq!(stdout, "configure 13 1 0\ntoggle 13\ntoggle 13\nidle\n");
    // `main` and the interrupt entry both use the state: its operations are critical sections.
    let dir = test_dir("static-state-shared");
    let (analysis, entry) = build_embedded(&dir, &[("main.emel", main)]);
    let c = std::fs::read_to_string(dir.join("out/main.c")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(entry.is_some(), "{:#?}", analysis.diagnostics);
    assert!(c.contains("emela_board_critical_enter();"), "{c}");
}

#[test]
fn static_states_need_compile_time_values() {
    let main = r#"import Std.State.{State}

@static
const START = start()

fn start() -> Int { 1 }

fn main() {
  (use State[Int]).set(2)
}
"#;
    // Int is a value type and `start()` is evaluated at compile time: this builds.
    let dir = test_dir("static-int");
    let (analysis, entry) = build_embedded(&dir, &[("main.emel", main)]);
    let c = std::fs::read_to_string(dir.join("out/main.c")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(entry.is_some(), "{:#?}", analysis.diagnostics);
    // Only `main` uses the state: no critical section.
    assert!(!c.contains("emela_board_critical_enter"), "{c}");
    let messages = errors(
        "import Std.State.{State}\n\n@static\nconst A = 1\n@static\nconst B = 2\n\nfn main() {\n  (use State[Int]).set(2)\n}\n",
        "static-twice",
    );
    assert!(
        messages.iter().any(|m| m.contains("two `@static` consts")),
        "{messages:#?}"
    );
}

#[test]
fn strings_live_in_fixed_buffers() {
    let main = r##"import Std.Embedded.Gpio.{Gpio, GpioPin, High}

type Reading(label: String, value: Int)

fn describe(r: Reading) -> String { "#{r.label}=#{r.value}" }

fn pin_for(name: String) -> Int {
  name match {
    "led" -> 13
    "button" -> 2
    _ -> 0
  }
}

fn repeat(s: String, n: Int) -> String {
  if n <= 0 { "" } else { "#{s}#{repeat(s, n - 1)}" }
}

fn main() {
  r = Reading(label: "temp", value: 21)
  text = describe(r)
  dbg(text)
  dbg(r)
  dbg(Some(String.trim("  x y  ")))
  dbg(Float.to_string(0.1 + 0.2))
  dbg("a" < "b")
  if String.starts_with(text, "temp") && String.contains(text, "=2") {
    dbg(pin_for("led") + String.byte_size(text))
    ()
  }
  dbg(String.byte_size(repeat("ab", 100)))
  if pin_for("x") == 0 { panic("bad #{repeat("abc", 100)}") }
}
"##;
    let Some((stdout, code)) = run(main, "strings", None) else {
        return;
    };
    assert_eq!(code, 3);
    assert_eq!(
        stdout,
        "dbg \"temp=21\"\n\
         dbg Reading(label: \"temp\", value: 21)\n\
         dbg Some(\"x y\")\n\
         dbg \"0.30000000000000004\"\n\
         dbg True\n\
         dbg 20\n\
         dbg 200\n\
         defect 7 string capacity exceeded\n"
    );
}

#[test]
fn strings_cross_into_c() {
    let main = r##"@external(embedded, "board_name")
fn board_name() -> String use {}

@external(embedded, "log_text")
fn log_text(text: String, times: Int) -> () use {}

fn main() {
  name = board_name()
  log_text("hello #{name}", 2)
}
"##;
    let board = r#"
uint16_t board_name(char *out, uint16_t capacity) {
  const char *name = "nucleo";
  uint16_t n = (uint16_t)strlen(name);
  if (n <= capacity) memcpy(out, name, n);
  return n;
}
void log_text(const char *text, uint16_t text_length, int32_t times) {
  for (int32_t i = 0; i < times; i++) printf("log %s (%d)\n", text, (int)text_length);
}
"#;
    let Some((stdout, code)) = run_project(
        &[("main.emel", main)],
        &format!("#include <string.h>\n{board}"),
        "strings-c",
        None,
    ) else {
        return;
    };
    assert_eq!(code, 0);
    assert_eq!(
        stdout,
        "log hello nucleo (12)\nlog hello nucleo (12)\nidle\n"
    );
}

#[test]
fn string_capacity_is_a_setting() {
    let main = r##"fn main() {
  s = "#{Int.to_string(1234)}#{Int.to_string(5678)}"
  dbg(String.byte_size(s))
  dbg("#{s}9")
}
"##;
    let toml = "[build]\ntarget = \"c-embedded\"\n\n[embedded.memory]\nstring_capacity = 8\n";
    let Some((stdout, code)) = run_project(
        &[("Pome.toml", toml), ("main.emel", main)],
        "",
        "capacity",
        None,
    ) else {
        return;
    };
    assert_eq!(code, 3);
    assert_eq!(stdout, "dbg 8\ndefect 7 string capacity exceeded\n");
    let dir = test_dir("capacity-bad");
    let bad = "[build]\ntarget = \"c-embedded\"\n\n[embedded.memory]\nstring_capacity = 1000\n";
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(dir.join("app/Pome.toml"), bad).unwrap();
    std::fs::write(dir.join("app/src/main.emel"), main).unwrap();
    let input = Input::resolve(&OsFs, &dir.join("app")).unwrap();
    let result = emela_driver::embedded_options(&OsFs, &input);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(result.is_err_and(|d| d.message.contains("512")));
}

/// `emela test` of a `c-embedded` project runs on the host C target: the board's effects are
/// replaced by test handlers, and strings longer than the board's capacity are defects there too.
#[test]
fn tests_of_embedded_projects_run_on_the_host() {
    if !have_cc() {
        return;
    }
    let main = r##"import Std.Embedded.Gpio.{Gpio, GpioPin, PinState, High, Low}
import Std.State.{State}

fn light_if_pressed(button: GpioPin, led: GpioPin) use { Gpio } {
  gpio = use Gpio
  gpio.read(button) match {
    Low -> gpio.write(led, High)
    High -> gpio.write(led, Low)
  }
}

fn label(n: Int) -> String { "count=#{n}" }

handler TestGpio implements Gpio {
  fn configure(pin, mode, pull) { () }
  fn read(pin) { Low }
  fn write(pin, state) {
    (use State[List[PinState]]).update(fn(xs) { List.append(xs, [state]) })
  }
  fn toggle(pin) { () }
  fn enable_interrupt(pin, edge) { () }
  fn disable_interrupt(pin) { () }
}

@test
fn lights_when_pressed() {
  (_, writes) = State.run([], fn() {
    with TestGpio { light_if_pressed(GpioPin(1), GpioPin(13)) }
  })
  assert writes == [High]
}

@test
fn short_label_fits() {
  assert label(7) == "count=7"
}

@test
fn long_label_does_not_fit() {
  assert String.byte_size("#{label(1234567890)}!") > 0
}

fn main() {
  light_if_pressed(GpioPin(2), GpioPin(13))
}
"##;
    let toml = "[build]\ntarget = \"c-embedded\"\n\n[embedded.memory]\nstring_capacity = 16\n";
    let dir = test_dir("host-tests");
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(dir.join("app/Pome.toml"), toml).unwrap();
    std::fs::write(dir.join("app/src/main.emel"), main).unwrap();
    let input = Input::resolve(&OsFs, &dir.join("app")).unwrap();
    let capacity = emela_driver::embedded_options(&OsFs, &input)
        .unwrap()
        .string_capacity;
    let options = TestOptions {
        out_dir: dir.join("out"),
        node: PathBuf::from("node"),
        filter: None,
        output: Output::Capture,
    };
    let mut out = Vec::new();
    let (analysis, report) = test(
        &OsFs,
        &input,
        &mut LowerTests::default()
            .with_target(Target::C)
            .for_embedded_project(),
        &mut CoreC::default().with_string_capacity(capacity),
        &options,
        |_| {},
        &mut out,
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!analysis.has_errors(), "{:#?}", analysis.diagnostics);
    let results: Vec<(String, TestOutcome)> = report
        .unwrap()
        .results
        .into_iter()
        .map(|r| (r.name, r.outcome))
        .collect();
    assert_eq!(
        results,
        [
            ("lights_when_pressed".to_owned(), TestOutcome::Passed),
            ("short_label_fits".to_owned(), TestOutcome::Passed),
            (
                "long_label_does_not_fit".to_owned(),
                TestOutcome::Defect("string capacity exceeded".to_owned())
            ),
        ]
    );
}

/// `emela test --device`: the tests run as a firmware through the project's runner (here the host
/// cc and the stub board), started again after a test that stops the board.
#[test]
fn tests_run_on_the_device_through_a_runner() {
    if !have_cc() {
        return;
    }
    let main = r##"import Std.Embedded.Gpio.{Gpio, GpioPin, PinState, High, Low}
import Std.State.{State}

fn light_if_pressed(button: GpioPin, led: GpioPin) use { Gpio } {
  gpio = use Gpio
  gpio.read(button) match {
    Low -> gpio.write(led, High)
    High -> gpio.write(led, Low)
  }
}

fn label(n: Int) -> String { "count=#{n}" }

handler TestGpio implements Gpio {
  fn configure(pin, mode, pull) { () }
  fn read(pin) { Low }
  fn write(pin, state) {
    (use State[List[PinState]]).update(fn(xs) { List.append(xs, [state]) })
  }
  fn toggle(pin) { () }
  fn enable_interrupt(pin, edge) { () }
  fn disable_interrupt(pin) { () }
}

@test
fn lights_when_pressed() {
  (_, writes) = State.run([], fn() {
    with TestGpio { light_if_pressed(GpioPin(1), GpioPin(13)) }
  })
  assert writes == [High]
}

@test
fn long_label_does_not_fit() {
  assert String.byte_size("#{label(1234567890)}!") > 0
}

@test
fn short_label_fits() {
  assert label(7) == "count=7"
}

fn main() {
  light_if_pressed(GpioPin(2), GpioPin(13))
}
"##;
    let toml = "[build]\ntarget = \"c-embedded\"\n\n[embedded.memory]\nstring_capacity = 16\nheap_bytes = 4096\n\n[embedded.test]\nrunner = \"runner.sh\"\n";
    let (cc, flags) = cc_command();
    let runner = format!(
        "#!/bin/sh\nset -e\ndir=\"$1\"\n'{}' {} -std=c11 -Wall -Wextra -Werror -O1 -I \"$dir\" \"$dir\"/*.c board.c -lm -o \"$dir/firmware\"\nexec \"$dir/firmware\"\n",
        cc.display(),
        flags.join(" ")
    );
    let dir = test_dir("device-tests");
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(dir.join("app/Pome.toml"), toml).unwrap();
    std::fs::write(dir.join("app/src/main.emel"), main).unwrap();
    std::fs::write(dir.join("app/board.c"), STUB_BOARD).unwrap();
    std::fs::write(dir.join("app/runner.sh"), runner).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            dir.join("app/runner.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    let input = Input::resolve(&OsFs, &dir.join("app")).unwrap();
    let device = emela_driver::DeviceTests {
        options: emela_driver::embedded_options(&OsFs, &input).unwrap(),
        runner: emela_driver::embedded_test_runner(&OsFs, &input)
            .unwrap()
            .unwrap(),
    };
    let options = TestOptions {
        out_dir: dir.join("out"),
        node: PathBuf::from("node"),
        filter: None,
        output: Output::Capture,
    };
    let mut out = Vec::new();
    let (analysis, report) = emela_driver::test_on_device(
        &OsFs,
        &input,
        &mut LowerTests::default()
            .with_target(Target::CEmbedded)
            .for_embedded_project(),
        &device,
        &options,
        |_| {},
        &mut out,
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!analysis.has_errors(), "{:#?}", analysis.diagnostics);
    let out = String::from_utf8_lossy(&out);
    let results: Vec<(String, TestOutcome)> = report
        .expect("a report")
        .results
        .into_iter()
        .map(|r| (r.name, r.outcome))
        .collect();
    assert_eq!(
        results,
        [
            ("lights_when_pressed".to_owned(), TestOutcome::Passed),
            (
                "long_label_does_not_fit".to_owned(),
                TestOutcome::Defect("string capacity exceeded".to_owned())
            ),
            ("short_label_fits".to_owned(), TestOutcome::Passed),
        ],
        "{out}"
    );
}

#[test]
fn failures_are_values() {
    let main = r##"import Std.Embedded.Gpio.{Gpio, GpioPin, High}

error OutOfRange(value: Int, limit: Int)
error Empty

fn checked(n: Int) -> Int fails OutOfRange | Empty {
  if n == 0 { fail Empty }
  if n > 10 { fail OutOfRange(value: n, limit: 10) }
  n * 2
}

fn describe(n: Int) -> Int fails Empty {
  checked(n) escape {
    OutOfRange(value:, limit:) -> value - limit
  }
}

fn main() -> () fails Empty {
  dbg(describe(3))
  dbg(describe(15))
  dbg(Int.parse("-42") escape { ParseError(_) -> 0 })
  dbg(Int.parse("4x") escape { ParseError(input:) -> String.byte_size(input) })
  dbg(Float.parse("2.5") escape { ParseError(_) -> 0.0 })
  dbg(describe(0))
  ()
}
"##;
    let Some((stdout, code)) = run(main, "failures", None) else {
        return;
    };
    assert_eq!(code, 3);
    assert_eq!(
        stdout,
        "dbg 6\ndbg 5\ndbg -42\ndbg 2\ndbg 2.5\ndefect 9 unhandled error\n"
    );
}

const F446: &str = include_str!("../../../boards/stm32f4/nucleo-f446re.toml");

/// A program with an interrupt entry (`button_changed`), for the board tests.
const BUTTON_PROGRAM: &str = "\
import Std.Embedded.Gpio.{Gpio, GpioPin, GpioEvent, OutputPushPull, NoPull, Input, Up, Both}

pub fn button_changed(event: GpioEvent) use { Gpio } {
  (use Gpio).write(GpioPin(13), event.level)
}

fn main() use { Gpio } {
  gpio = use Gpio
  gpio.configure(GpioPin(13), OutputPushPull, NoPull)
  gpio.configure(GpioPin(100), Input, Up)
  gpio.enable_interrupt(GpioPin(100), Both)
}
";

/// With a board definition, the program's pins are checked against it and the HAL adapter is
/// generated with the bound interrupts.
#[test]
fn boards_check_pins_and_generate_the_adapter() {
    let toml = "[build]\ntarget = \"c-embedded\"\nboard = \"board.toml\"\n\n\
                [embedded.gpio_interrupts]\n100 = \"button_changed\"\n";
    let dir = test_dir("board");
    std::fs::create_dir_all(dir.join("app")).unwrap();
    std::fs::write(dir.join("app/board.toml"), F446).unwrap();
    let (analysis, entry) =
        build_embedded(&dir, &[("Pome.toml", toml), ("main.emel", BUTTON_PROGRAM)]);
    assert!(entry.is_some(), "{:#?}", analysis.diagnostics);
    let adapter = std::fs::read_to_string(dir.join("out/emela_board_hal.c")).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(adapter.contains("case 13: /* D13 PA5 */"), "{adapter}");
    assert!(
        adapter.contains("void EXTI15_10_IRQHandler(void)"),
        "{adapter}"
    );
    assert!(
        adapter.contains("emela_export_button_changed(event);"),
        "{adapter}"
    );

    let wrong = |main: &str, toml: &str, name: &str| -> Vec<String> {
        let dir = test_dir(name);
        std::fs::create_dir_all(dir.join("app")).unwrap();
        std::fs::write(dir.join("app/board.toml"), F446).unwrap();
        let (analysis, entry) = build_embedded(&dir, &[("Pome.toml", toml), ("main.emel", main)]);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(entry.is_none());
        analysis
            .diagnostics
            .iter()
            .map(|d| d.message.clone())
            .collect()
    };
    let board_only = "[build]\ntarget = \"c-embedded\"\nboard = \"board.toml\"\n";
    let unknown = wrong(
        "import Std.Embedded.Gpio.{Gpio, GpioPin, High}\nfn main() { (use Gpio).write(GpioPin(42), High) }\n",
        board_only,
        "board-unknown-pin",
    );
    assert!(
        unknown.iter().any(|m| m.contains("logical pin 42")),
        "{unknown:#?}"
    );
    let computed = wrong(
        "import Std.Embedded.Gpio.{Gpio, GpioPin, High}\nfn on(n: Int) use { Gpio } { (use Gpio).write(GpioPin(n + 1), High) }\nfn main() { on(3) }\n",
        board_only,
        "board-computed-pin",
    );
    assert!(
        computed.iter().any(|m| m.contains("constants")),
        "{computed:#?}"
    );
    let bad_handler = wrong(
        BUTTON_PROGRAM,
        "[build]\ntarget = \"c-embedded\"\nboard = \"board.toml\"\n\n[embedded.gpio_interrupts]\n100 = \"missing\"\n9 = \"button_changed\"\n11 = \"button_changed\"\n",
        "board-bad-handler",
    );
    assert!(
        bad_handler.iter().any(|m| m.contains("`missing`")),
        "{bad_handler:#?}"
    );
    let conflict = wrong(
        "import Std.Embedded.Gpio.{Gpio, GpioPin, High}\n\
         import Std.Embedded.Spi.{Spi, Spi1}\n\
         import Std.Embedded.Peripheral.{PeripheralError}\n\
         fn main() {\n  (use Gpio).write(GpioPin(13), High)\n  \
         (use Spi).transmit(Spi1, [1], Int64.from_int(1)) escape { PeripheralError(_) -> () }\n}\n",
        "[build]\ntarget = \"c-embedded\"\nboard = \"board.toml\"\n\n[embedded.memory]\nheap_bytes = 512\n",
        "board-conflict",
    );
    assert!(
        conflict.iter().any(|m| m.contains("sck of Spi1")),
        "{conflict:#?}"
    );
    // D9 (PC7) and D11 (PA7) share EXTI7.
    assert!(
        bad_handler.iter().any(|m| m.contains("EXTI7")),
        "{bad_handler:#?}"
    );
}

#[test]
fn platform_handlers_use_the_board() {
    let main = r##"import Std.Embedded.Gpio.{Gpio, GpioPin}
import Std.Platform.{Platform}
import Std.Clock.{Clock}
import Std.Console.{Console}
import Std.Instant.{Instant}
import Std.Random.{Random}

fn blink(n: Int) -> () use { Gpio, Clock, Console } {
  if n < 2 {
    (use Gpio).toggle(GpioPin(13))
    (use Console).print_line("toggle #{n}")
    (use Clock).sleep(500)
    blink(n + 1)
  }
}

fn main() {
  with Platform {
    blink(0)
    dbg(Instant.to_unix_milliseconds((use Clock).now()))
    dbg((use Random).int())
    dbg((use Random).float())
    (use Console).print_error_line("done")
  }
}
"##;
    let Some((stdout, code)) = run(main, "platform", None) else {
        return;
    };
    assert_eq!(code, 0);
    assert_eq!(
        stdout,
        "toggle 13\ntoggle 0\nsleep 500\ntoggle 13\ntoggle 1\nsleep 500\n\
         dbg 1000\ndbg 7\ndbg 0.5\nerr:doneerr:\nidle\n"
    );
}

/// An interpolation given to Console is written in pieces, not built: the string capacity does
/// not limit it.
#[test]
fn console_lines_are_written_in_pieces() {
    let main = r##"import Std.Platform.{Platform}
import Std.Console.{Console}

fn main() {
  with Platform {
    (use Console).print_line("a long line: #{12345678} and #{-9}")
  }
}
"##;
    let toml = "[build]\ntarget = \"c-embedded\"\n\n[embedded.memory]\nstring_capacity = 8\n";
    let Some((stdout, code)) = run_project(
        &[("Pome.toml", toml), ("main.emel", main)],
        "",
        "console-pieces",
        None,
    ) else {
        return;
    };
    assert_eq!(code, 0);
    assert_eq!(stdout, "a long line: 12345678 and -9\nidle\n");
}

/// With `heap_bytes`, lists and recursive enums live in boxes on the heap (outside interrupts);
/// running out of it is a defect.
#[test]
fn heap_values_live_in_fixed_boxes() {
    let main = r##"enum Tree { Leaf, Node(Tree, Int, Tree) }

fn size(t: Tree) -> Int {
  t match {
    Leaf -> 0
    Node(l, _, r) -> size(l) + 1 + size(r)
  }
}

fn build(n: Int) -> Tree {
  if n <= 0 { Leaf } else { Node(build(n - 1), n, Leaf) }
}

## Makes and drops a list each time: the boxes go back to the heap.
fn churn(n: Int, total: Int) -> Int {
  if n <= 0 { total } else { churn(n - 1, total + List.length([n, n, n])) }
}

fn main() {
  xs = [1, 2, 3]
  ys = List.map(xs, fn(x) { x * 10 })
  dbg(List.length(ys))
  dbg(ys)
  dbg(List.reverse(ys))
  dbg(size(Node(Leaf, 1, Node(Leaf, 2, Leaf))))
  names = ["a", "bc"]
  dbg(List.map(names, fn(s) { "#{s}!" }))
  dbg(size(build(10)))
  dbg(churn(10000, 0))
  dbg(size(build(10000)))
  ()
}
"##;
    let toml = "[build]\ntarget = \"c-embedded\"\n\n[embedded.memory]\nheap_bytes = 8192\n";
    let Some((stdout, code)) = run_project(
        &[("Pome.toml", toml), ("main.emel", main)],
        "",
        "heap-boxes",
        None,
    ) else {
        return;
    };
    assert_eq!(code, 3);
    assert_eq!(
        stdout,
        "dbg 3\ndbg [10, 20, 30]\ndbg [30, 20, 10]\ndbg 2\ndbg [\"a!\", \"bc!\"]\ndbg 10\ndbg 30000\n\
         defect 8 heap exhausted\n"
    );
    let messages = errors("fn main() {\n  dbg([1, 2])\n  ()\n}\n", "no-heap");
    assert!(
        messages.iter().any(|m| m.contains("heap_bytes")),
        "{messages:#?}"
    );
    // Not in interrupts, even with a heap.
    let dir = test_dir("heap-isr");
    let isr = "import Std.Embedded.Gpio.{Gpio, GpioEvent}\n\
               pub fn changed(event: GpioEvent) use { Gpio } {\n  dbg(List.length([event.pin.id]))\n  ()\n}\n\
               fn main() { () }\n";
    let (analysis, entry) = build_embedded(&dir, &[("Pome.toml", toml), ("main.emel", isr)]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(entry.is_none());
    assert!(
        analysis
            .diagnostics
            .iter()
            .any(|d| d.message.contains("interrupts cannot use the heap")),
        "{:#?}",
        analysis.diagnostics
    );
}

/// UART (and the other peripherals) take and give `List[Int]` through arrays, and their failures
/// are `PeripheralError(code)` from the board's error code.
#[test]
fn peripherals_pass_lists_and_fail_with_codes() {
    let main = r##"import Std.Embedded.Uart.{Uart, Uart0, Uart1, UartConfig, Bits8, One, NoParity, Both, NoFlowControl, By16}
import Std.Embedded.Peripheral.{PeripheralError}

fn exchange() -> Int fails PeripheralError use { Uart } {
  uart = use Uart
  config = UartConfig(
    baud_rate: 115200,
    data_bits: Bits8,
    stop_bits: One,
    parity: NoParity,
    direction: Both,
    flow_control: NoFlowControl,
    oversampling: By16
  )
  timeout = Int64.from_int(100)
  uart.configure(Uart0, config)
  uart.transmit(Uart0, [72, 105], timeout)
  dbg(uart.receive(Uart0, 3, timeout))
  uart.transmit(Uart0, [70000], timeout)
  0
}

fn other() -> Int fails PeripheralError use { Uart } {
  (use Uart).transmit(Uart1, [1], Int64.from_int(10))
  0
}

fn main() {
  dbg(exchange() escape { PeripheralError(code:) -> code })
  dbg(other() escape { PeripheralError(code:) -> code })
  ()
}
"##;
    let board = r#"
#ifdef EMELA_UART_H
int32_t emela_uart_configure(EmelaUartId id, EmelaUartConfig config) {
  printf("configure %d %d\n", (int)id, (int)config.baud_rate);
  return 0;
}
int32_t emela_uart_transmit(EmelaUartId id, const uint16_t *data, uint16_t data_length, int64_t timeout) {
  (void)timeout;
  if (id != EMELA_UART_ID_UART0) return 5;
  printf("transmit");
  for (uint16_t i = 0; i < data_length; i++) printf(" %d", (int)data[i]);
  printf("\n");
  return 0;
}
int32_t emela_uart_receive(EmelaUartId id, int32_t size, int64_t timeout, uint16_t *out, uint16_t capacity, uint16_t *length) {
  (void)id; (void)timeout;
  *length = 0;
  for (int32_t i = 0; i < size && i < capacity; i++) out[(*length)++] = (uint16_t)(i + 1);
  return 0;
}
#endif
"#;
    let toml = "[build]\ntarget = \"c-embedded\"\n\n[embedded.memory]\nheap_bytes = 1024\n";
    let Some((stdout, code)) = run_project(
        &[("Pome.toml", toml), ("main.emel", main)],
        board,
        "uart",
        None,
    ) else {
        return;
    };
    assert_eq!(code, 0);
    assert_eq!(
        stdout,
        "configure 0 115200\ntransmit 72 105\ndbg [1, 2, 3]\ndbg -1\ndbg 5\nidle\n"
    );
}

/// The adapter covers the UART, I2C and SPI routes of the board for the peripherals the program
/// uses.
#[test]
fn adapters_follow_the_peripheral_routes() {
    let main = r##"import Std.Embedded.Uart.{Uart, Uart0}
import Std.Embedded.I2c.{I2c, I2c1}
import Std.Embedded.Spi.{Spi, Spi1}
import Std.Embedded.Peripheral.{PeripheralError}

fn run() -> Bool fails PeripheralError use { Uart, I2c, Spi } {
  t = Int64.from_int(10)
  (use Uart).transmit(Uart0, [1], t)
  (use Spi).transmit(Spi1, [2], t)
  (use I2c).is_device_ready(I2c1, 72, 1, t)
}

fn main() {
  dbg(run() escape { PeripheralError(_) -> False })
  ()
}
"##;
    let toml = "[build]\ntarget = \"c-embedded\"\nboard = \"board.toml\"\n\n[embedded.memory]\nheap_bytes = 1024\n";
    let dir = test_dir("adapters");
    std::fs::create_dir_all(dir.join("app")).unwrap();
    std::fs::write(dir.join("app/board.toml"), F446).unwrap();
    let (analysis, entry) = build_embedded(&dir, &[("Pome.toml", toml), ("main.emel", main)]);
    assert!(entry.is_some(), "{:#?}", analysis.diagnostics);
    let adapter = std::fs::read_to_string(dir.join("out/emela_board_hal.c")).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    for expected in [
        "case EMELA_UART_ID_UART0: /* USART2 */",
        "case EMELA_I2C_ID_I2C1: /* I2C1 */",
        "case EMELA_SPI_ID_SPI1: /* SPI1 */",
        "int32_t emela_i2c_is_device_ready(",
        "int32_t emela_spi_transfer(",
    ] {
        assert!(adapter.contains(expected), "{expected}\n{adapter}");
    }
    assert!(!adapter.contains("emela_gpio_write"), "{adapter}");
}

/// With a heap, the String functions that take or give lists work too.
/// Exported functions take Strings as pointer and length, and write a String result to the
/// caller's buffer.
#[test]
fn exported_functions_pass_strings() {
    let main = r##"pub fn greet(name: String) -> String { "hello #{name}" }

pub fn size(s: String) -> Int { String.byte_size(s) }

pub fn echo(s: String) -> String { s }

fn main() { () }
"##;
    let events = r#"char buf[16]; uint16_t n = emela_export_greet("bob", 3, buf, sizeof buf);
printf("%.*s|%s\n", (int)n, buf, buf);
printf("%d\n", (int)emela_export_size("h\xc3\xa9llo", 6));
n = emela_export_echo("xy", 2, buf, 2);
printf("%.*s\n", (int)n, buf);
emela_export_greet("bob", 3, buf, 4);"#;
    let Some((stdout, code)) = run(main, "export-strings", Some(&events.replace('\n', " "))) else {
        return;
    };
    assert_eq!(code, 3, "{stdout}");
    assert_eq!(
        stdout,
        "hello bob|hello bob\n6\nxy\ndefect 7 string capacity exceeded\n"
    );
}

/// Freeing a long chain of boxes does not recurse, and the heap's usage can be read.
#[test]
fn heap_frees_deep_values_in_a_loop() {
    let main = r##"enum Chain { End, Link(Int, Chain) }
enum Tree { Leaf, Node(Tree, Int, Tree) }

fn chain(n: Int, acc: Chain) -> Chain {
  if n <= 0 { acc } else { chain(n - 1, Link(n, acc)) }
}

fn first(c: Chain) -> Int {
  c match {
    End -> 0
    Link(n, _) -> n
  }
}

fn root(t: Tree) -> Int {
  t match {
    Leaf -> 0
    Node(_, n, _) -> n
  }
}

fn main() {
  dbg(first(chain(50000, End)))
  t = Node(Node(Leaf, 1, Leaf), 2, Node(Leaf, 3, Leaf))
  nested = [[1, 2], [3], []]
  dbg(List.length(nested))
  dbg(root(t))
  ()
}
"##;
    let toml = "[build]\ntarget = \"c-embedded\"\n\n[embedded.memory]\nheap_bytes = 2000000\n";
    let events = r#"printf("live %u used %d\n", (unsigned)emela_heap_live_bytes(), emela_heap_used_bytes() >= 50000 * 16);"#;
    let dir = test_dir("heap-drain-c");
    let (analysis, entry) = build_embedded(&dir, &[("Pome.toml", toml), ("main.emel", main)]);
    let c = std::fs::read_to_string(dir.join("out/main.c")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(entry.is_some(), "{:#?}", analysis.diagnostics);
    // The drop functions only hand boxes to the loop.
    let drain = &c[c.find("static void em_drain(void) {").expect("em_drain")..];
    let drain = &drain[..drain.find("\n}\n").unwrap()];
    assert!(!drain.contains("em_drop_"), "{drain}");
    let Some((stdout, code)) = run_project(
        &[("Pome.toml", toml), ("main.emel", main)],
        "",
        "heap-drain",
        Some(events),
    ) else {
        return;
    };
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(stdout, "dbg 1\ndbg 3\ndbg 2\nlive 0 used 1\nidle\n");
}

#[test]
fn string_lists_use_the_heap() {
    let main = r##"fn main() {
  parts = String.split("a,bb,,c", ",")
  dbg(parts)
  dbg(String.join(List.reverse(parts), "-"))
  dbg(String.split("ab\r\nc", ""))
  dbg(String.chars("x\r\ny"))
  dbg(String.length("ab\r\nc"))
  dbg(String.code_points("aé"))
  dbg(String.from_code_point(233))
  dbg(String.from_code_point(-1))
  ()
}
"##;
    let toml = "[build]\ntarget = \"c-embedded\"\n\n[embedded.memory]\nheap_bytes = 8192\n";
    let Some((stdout, code)) = run_project(
        &[("Pome.toml", toml), ("main.emel", main)],
        "",
        "string-lists",
        None,
    ) else {
        return;
    };
    assert_eq!(code, 0);
    assert_eq!(
        stdout,
        "dbg [\"a\", \"bb\", \"\", \"c\"]\ndbg \"c--bb-a\"\ndbg [\"a\", \"b\", \"\\r\\n\", \"c\"]\n\
         dbg [\"x\", \"\\r\\n\", \"y\"]\ndbg 4\ndbg [97, 233]\ndbg Some(\"é\")\ndbg None\nidle\n"
    );
}
