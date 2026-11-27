//! Errors, handlers, stateful handlers and suspension through the `emela run` path.

use emela_core::build::*;
use emela_core::{BinOp, Builtin, Expr, OpTy, Type, TypeId, WithBind};
use emela_driver::{DEFECT_EXIT_CODE, INTERRUPTED_EXIT_CODE, RunOutput};

use super::{run_ir, run_source, text};

#[test]
fn unhandled_error_exits_with_1() {
    // An error that reaches main prints `error: <the error shown>` and exits with 1 (spec 14.3),
    // distinct from 101 for a defect.
    let output = run_source(
        "lower-fail",
        "\
error NotFound(id: Int) derive Show
error Unauthorized

fn find(id: Int) -> Int fails NotFound | Unauthorized {
  if id < 0 { fail Unauthorized } else if id == 0 { fail NotFound(id:) } else { id }
}

fn main() {
  dbg(find(1))
  find(0)
  ()
}
",
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (1, "", "1\nerror: NotFound(id: 0)\n")
    );
    let output = run_source(
        "lower-fail-unit",
        "error Unauthorized\n\nfn main() {\n  fail Unauthorized\n}\n",
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (1, "", "error: Unauthorized\n")
    );
    let output = run_source("lower-defect", "fn main() {\n  panic(\"boom\")\n}\n");
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (DEFECT_EXIT_CODE, "", "defect: boom\n")
    );
}

#[test]
fn errors_are_values_and_escape_recovers() {
    // Error values are ordinary values, shown by interpolation through Show and by dbg (spec 7.1,
    // 10.6). An error caught by escape becomes a normal value, and an uncaught one propagates.
    // Errors inside arms propagate too (spec 7.3, 14.3).
    let output = run_source(
        "lower-escape",
        "\
error NotFound(id: Int)
error DbError(message: String)
error Unavailable(reason: String)

impl NotFound: Show {
  fn show(self) { \"NotFound(id: #{self.id})\" }
}

impl NotFound: Eq {
  fn eq(self, other) { self.id == other.id }
}

impl DbError: Show {
  fn show(self) { \"DbError(message: \\\"#{self.message}\\\")\" }
}

fn load(id: Int) -> String fails NotFound | DbError {
  if id < 0 { fail DbError(message: \"down\") } else if id == 0 { fail NotFound(id:) } else { \"user #{id}\" }
}

fn partial(id: Int) {
  load(id) escape { NotFound(_) -> \"guest\" }
}

fn convert(id: Int) -> String fails NotFound | Unavailable {
  load(id) escape { DbError(message:) -> fail Unavailable(reason: message) }
}

fn main() {
  e = NotFound(id: 7)
  dbg(e)
  dbg(\"got #{e} and #{[DbError(message: \"x\")]}\")
  dbg(e == NotFound(id: 7))
  dbg(load(1))
  dbg(load(0) escape { NotFound(id:) -> \"none #{id}\" })
  dbg(Some(load(0)) escape { NotFound(_) -> None })
  dbg(partial(0))
  dbg(partial(-1) escape { DbError(message:) -> \"partial: #{message}\" })
  dbg(convert(-1) escape { Unavailable(reason:) -> \"unavailable: #{reason}\" })
  dbg([1, 2] |> List.get(5) |> Option.or_fail(NotFound(id: 5)) escape { NotFound(id:) -> id * 10 })
  dbg(convert(0))
  ()
}
",
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (
            1,
            "",
            "\
NotFound(id: 7)
\"got NotFound(id: 7) and [DbError(message: \\\"x\\\")]\"
True
\"user 1\"
\"none 0\"
None
\"guest\"
\"partial: down\"
\"unavailable: down\"
50
error: NotFound(id: 0)
"
        )
    );
}

#[test]
fn partial_escape_arms_propagate_unmatched_errors() {
    // An error missing arms that catch only some values (field values, guards) propagates as an
    // error, not a defect (spec 7.3). Reaching main gives `error: ...` and exit code 1, with only
    // the name of an error without Show (spec 14.3).
    let source = |last: &str| {
        format!(
            "\
error NotFound(id: Int)

fn find(id: Int) -> Int fails NotFound {{
  fail NotFound(id:)
}}

fn by_value(id: Int) {{
  find(id) escape {{ NotFound(id: 1) -> 10 }}
}}

fn by_guard(id: Int) {{
  find(id) escape {{ NotFound(id:) if id > 5 -> id * 10 }}
}}

fn main() {{
  dbg(by_value(1))
  dbg(by_guard(6))
  dbg(by_value(2) escape {{ NotFound(id:) -> id + 100 }})
  {last}
  ()
}}
"
        )
    };
    let output = run_source("lower-partial-escape-value", &source("by_value(2)"));
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (1, "", "10\n60\n102\nerror: NotFound\n")
    );
    let output = run_source("lower-partial-escape-guard", &source("by_guard(3)"));
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (1, "", "10\n60\n102\nerror: NotFound\n")
    );
}

#[test]
fn failing_tail_calls_do_not_grow_stack() {
    // Fallible self tail calls also become loops (spec 16.3). Calls inside an escape's target are
    // not in tail position.
    let output = run_source(
        "lower-fail-loop",
        "\
error Found(at: Int)

fn search(n: Int, target: Int) -> Int fails Found {
  if n == target { fail Found(at: n) } else if n == 0 { -1 } else { search(n - 1, target) }
}

fn main() {
  dbg(search(1000000, 3) escape { Found(at:) -> at })
  dbg(search(1000000, -5))
  ()
}
",
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "", "3\n-1\n")
    );
}

#[test]
fn console_runs_through_handlers() {
    // The bundled Console is provided by SystemConsole (spec 8.3, 15.2). An `Option.or_fail`
    // failure goes through as well.
    let output = run_source(
        "console",
        r#"import Std.Console.{Console, SystemConsole}

error NotFound(id: Int)

fn name(id: Int) -> String fails NotFound {
  List.get(["alice"], id) |> Option.or_fail(NotFound(id:))
}

fn main() {
  with SystemConsole {
    console = use Console
    console.print_line(name(0))
    console.print_line(name(1) escape { NotFound(id:) -> "no #{id}" })
    (use Console).print_error_line("done")
  }
}
"#,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "alice\nno 1\n", "done\n")
    );
}

#[test]
fn label_provider_uses_a_handler_of_the_same_with() {
    // In `with SystemConsole, AuditLog by PrefixConsole`, the Console that PrefixConsole uses is
    // SystemConsole in the same `with` (`[effect.label]`, issue #679).
    let output = run_source(
        "label-provider-same-with",
        r#"import Std.Console.{Console, SystemConsole}

effect AuditLog of Console

handler PrefixConsole: Console {
  fn print_line(text) { (use Console).print_line("[audit] #{text}") }
  fn print_error_line(text) { (use Console).print_error_line("[audit] #{text}") }
}

fn main() {
  with SystemConsole, AuditLog by PrefixConsole {
    (use AuditLog).print_line("x")
    (use AuditLog).print_error_line("y")
  }
}
"#,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "[audit] x\n", "[audit] y\n")
    );
}

/// Declarations for end-to-end checks of stateful handlers and layers (spec 8.4, 8.5). `Pool`
/// prints to `Console` on init and release. `Fragile` panics on release.
const STATEFUL: &str = r#"import Std.Console.{Console, SystemConsole}
import Std.Clock.{Clock, SystemClock}

error DbDown

effect Users {
  fn find(id: Int) -> String
}

effect Config {
  fn db_url() -> String
}

effect Wait {
  suspend fn forever() -> ()
}

handler EnvConfig: Config {
  fn db_url() { "db://app" }
}

handler EmptyConfig: Config {
  fn db_url() { "" }
}

handler Pool(url: String, opened: Int): Users {
  init {
    url = (use Config).db_url()
    if url == "" { fail DbDown }
    (use Console).print_line("open #{url}")
    Pool(url:, opened: 1)
  }

  release {
    (use Console).print_line("close #{self.url}")
  }

  fn find(id) { "user #{id} via #{self.url}" }
}

handler Spare: Users {
  init {
    (use Console).print_line("open spare")
    Spare
  }

  release { (use Console).print_line("close spare") }

  fn find(id) { "spare #{id}" }
}

handler Fragile: Wait {
  release { panic("close failed") }
  suspend fn forever(resume) { resume(()) }
}

handler Stuck: Wait {
  suspend fn forever(resume) { () }
}

layer AppLive {
  Pool
  EnvConfig
}
"#;

fn run_stateful(name: &str, main: &str) -> RunOutput {
    run_source(name, &format!("{STATEFUL}\n{main}"))
}

#[test]
fn stateful_handlers_and_layers_run() {
    // The layer is expanded; init runs in dependency order (EnvConfig, Pool) and release in
    // reverse (spec 8.5, 14.2). A handler overridden by a later one (AppLive's Pool) is not
    // initialized (spec 8.3).
    let output = run_stateful(
        "stateful",
        r#"fn main() {
  with SystemConsole, AppLive {
    (use Console).print_line((use Users).find(1))
  }
  with SystemConsole, AppLive, Spare {
    (use Console).print_line((use Users).find(2))
  }
}
"#,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (
            0,
            "open db://app\nuser 1 via db://app\nclose db://app\n\
             open spare\nspare 2\nclose spare\n",
            ""
        )
    );
}

#[test]
fn failing_init_releases_what_was_initialized() {
    // If Pool's init fails, the body does not run and only the already initialized Spare is
    // released (spec 14.2). The `with` ends with that init error.
    let output = run_stateful(
        "stateful-init-fails",
        r#"handler SpareFirst: Config {
  init {
    (use Console).print_line("open config")
    SpareFirst
  }
  release { (use Console).print_line("close config") }
  fn db_url() { "" }
}

fn main() {
  with SystemConsole, SpareFirst, Pool {
    (use Console).print_line("body")
  }
}
"#,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (1, "open config\nclose config\n", "error: DbDown\n")
    );
}

#[test]
fn release_defects_follow_the_body() {
    // If the body is a value, the release defect is the result. The remaining releases still run
    // (spec 14.4).
    let output = run_stateful(
        "stateful-release-defect",
        r#"fn main() {
  with SystemConsole, EnvConfig, Pool, Fragile {
    (use Wait).forever()
  }
}
"#,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (
            DEFECT_EXIT_CODE,
            "open db://app\nclose db://app\n",
            "defect: close failed\n"
        )
    );

    // If the body is a defect, the first defect is the result and the release defect becomes a
    // note.
    let output = run_stateful(
        "stateful-release-note",
        r#"fn main() {
  with SystemConsole, Fragile { panic("boom") }
}
"#,
    );
    assert_eq!(
        (output.code, text(&output.stderr)),
        (
            DEFECT_EXIT_CODE,
            "defect: boom\nnote: the release of handler Fragile also failed: close failed\n"
        )
    );
}

#[test]
fn interrupted_main_releases_first() {
    // A main unwound while suspended also runs the releases of open `with`s from the inside out,
    // then ends as `interrupted` (spec 14.5). The same holds for a non-suspending main ($sync) and
    // a main that could be resumed later.
    for (name, main) in [
        (
            "stateful-interrupted",
            "fn main() {\n  with SystemConsole, AppLive, Stuck { (use Wait).forever() }\n}\n",
        ),
        (
            "stateful-interrupted-async",
            "fn main() {\n  with SystemConsole, SystemClock, AppLive, Stuck {\n    (use Clock).sleep(1)\n    (use Wait).forever()\n  }\n}\n",
        ),
    ] {
        let output = run_stateful(name, main);
        assert_eq!(
            (output.code, text(&output.stdout), text(&output.stderr)),
            (
                INTERRUPTED_EXIT_CODE,
                "open db://app\nclose db://app\n",
                "interrupted\n"
            ),
            "{name}"
        );
    }
}

#[test]
fn lists_strings_and_int64_run_through_core() {
    // take, drop, sort_by (stable) and sort; ASCII case changes only A to Z and a to z; Int64's
    // compare, min, max and abs.
    let output = run_source(
        "core_additions",
        r##"fn main() {
  xs = [3, 1, 2]
  dbg((List.take(xs, 2), List.drop(xs, 2), List.take(xs, -1), List.drop(xs, 5)))
  none: List[Int] = []
  dbg((List.last(xs), List.last(none), List.find_index(xs, |x| x == 2)))
  dbg(List.flat_map(xs, |x| [x, x]))
  dbg(List.filter_map(xs, |x| if x > 1 { Some(x * 10) } else { None }))
  dbg(List.enumerate(["a", "b"]))
  dbg(List.unzip([(1, "a"), (2, "b")]))
  pairs = [(2, "a"), (1, "b"), (2, "c"), (1, "d")]
  dbg(List.sort_by(pairs, |a, b| {
    (x, _) = a
    (y, _) = b
    Ord.compare(x, y)
  }))
  dbg(List.sort([3, 1, 2, 1]))
  dbg(List.sort([Some(2), None, Some(1)]))
  dbg(String.to_ascii_lowercase("Hello, ÄB 東 @[`{"))
  dbg(String.to_ascii_uppercase("Hello, äb 東 @[`{"))
  big = Int64.from_int(2147483647) * Int64.from_int(4)
  one = Int64.from_int(1)
  dbg((big.compare(one), Int64.min(big, one), Int64.max(big, one), Int64.abs(-big)))
  ()
}
"##,
    );
    assert_eq!(output.code, 0);
    assert_eq!(
        text(&output.stderr),
        r#"([3, 1], [2], [], [])
(Some(2), None, Some(2))
[3, 3, 1, 1, 2, 2]
[30, 20]
[(0, "a"), (1, "b")]
([1, 2], ["a", "b"])
[(1, "b"), (1, "d"), (2, "a"), (2, "c")]
[1, 1, 2, 3]
[None, Some(1), Some(2)]
"hello, Äb 東 @[`{"
"HELLO, äB 東 @[`{"
(Greater, 1, 8589934588, 8589934588)
"#
    );
}

#[test]
fn time_values_run_through_core() {
    // An Instant can be made from milliseconds since the Unix epoch (spec 15.2).
    let output = run_source(
        "time",
        r#"import Std.Console.{Console, SystemConsole}
import Std.Instant.{Instant}
import Std.Duration.{Duration}

fn show(o: Ordering) -> String {
  o match {
    Less -> "Less"
    Equal -> "Equal"
    Greater -> "Greater"
  }
}

fn main() {
  with SystemConsole {
    c = use Console
    d = Duration.add(Duration.minutes(2), Duration.seconds(3))
    c.print_line(Duration.to_string(d))
    c.print_line(Int64.to_string(Duration.to_seconds(Duration.milliseconds(-1500))))
    c.print_line(Duration.to_string(Duration.subtract(Duration.hours(1), Duration.minutes(61))))
    c.print_line(show(Duration.seconds(1).compare(Duration.milliseconds(1000))))
    c.print_line(if Duration.seconds(1) == Duration.milliseconds(1000) { "eq" } else { "ne" })
    start = Instant.from_unix_milliseconds(1760238245678)
    later = start |> Instant.add(Duration.hours(25))
    c.print_line(Instant.to_string(start))
    c.print_line(Instant.to_string(later))
    c.print_line(Duration.to_string(later |> Instant.since(start)))
    c.print_line(show(start.compare(later)))
    c.print_line(Int64.to_string(Instant.to_unix_milliseconds(Instant.subtract(start, Duration.milliseconds(678)))))
    c.print_line(Instant.to_string(Instant.from_unix_milliseconds(0)))
    c.print_line(Instant.to_string(Instant.from_unix_milliseconds(-1)))
    c.print_line(Instant.to_string(Instant.from_unix_milliseconds(951782400000)))
    c.print_line(Instant.to_string(Instant.from_unix_milliseconds(-62198755200000)))
    c.print_line(Instant.to_string(Instant.from_unix_milliseconds(253402300800000)))
  }
}
"#,
    );
    assert_eq!(text(&output.stderr), "");
    assert_eq!(output.code, 0);
    insta::assert_snapshot!(text(&output.stdout));
}

use crate::runtime::suspend;

#[test]
fn suspended_main_waits_for_promises() {
    // A suspended main waits on the event loop, resumes and finishes.
    let mut m = emela_core::Module::new();
    let resolve = suspend::resolve(&mut m);
    suspend::main(
        &mut m,
        TypeId::INT,
        builtin_ty(
            Builtin::Dbg,
            vec![TypeId::INT],
            vec![bin(
                BinOp::Add,
                OpTy::Int,
                call_suspends(resolve, vec![int(41)]),
                int(1),
            )],
        ),
    );
    let output = run_ir("suspend-promise", m);
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "", "42\n")
    );
}

#[test]
fn dropped_resume_interrupts_main() {
    // A main that ends without a resume gives `interrupted` and exit code 130 (spec 14.5).
    let mut m = emela_core::Module::new();
    let (never, wait) = suspend::never(&mut m);
    let h = m.local("h", Type::Handler(wait));
    suspend::main(
        &mut m,
        TypeId::UNIT,
        Expr::With {
            binds: vec![WithBind::new(h, call(never, vec![]))],
            body: Box::new(suspend::forever(wait, h)),
        },
    );
    let output = run_ir("suspend-interrupted", m);
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (INTERRUPTED_EXIT_CODE, "", "interrupted\n")
    );
}

#[test]
fn dropped_main_releases_before_interrupted() {
    // A main unwound while suspended also runs the releases of open `with`s, then ends as
    // `interrupted` (spec 14.5).
    let mut m = emela_core::Module::new();
    let (never, wait) = suspend::never(&mut m);
    let log = suspend::log(&mut m);
    let (h, r) = (
        m.local("h", Type::Handler(wait)),
        m.local("r", Type::Handler(wait)),
    );
    let pool = suspend::released(&mut m, log, r, "Pool", "released");
    suspend::main(
        &mut m,
        TypeId::UNIT,
        Expr::With {
            binds: vec![pool, WithBind::new(h, call(never, vec![]))],
            body: Box::new(suspend::forever(wait, h)),
        },
    );
    let output = run_ir("release-interrupted", m);
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (INTERRUPTED_EXIT_CODE, "released\n", "interrupted\n")
    );
}

#[test]
fn defects_after_resuming_and_in_callbacks() {
    // A defect after resuming and a defect inside a host callback both end as defects
    // (spec 7.5).
    let mut m = emela_core::Module::new();
    let resolve = suspend::resolve(&mut m);
    let x = m.local("_", TypeId::INT);
    suspend::main(
        &mut m,
        TypeId::NEVER,
        let_(
            x,
            call_suspends(resolve, vec![int(1)]),
            builtin(Builtin::Panic, vec![string("after")]),
        ),
    );
    let output = run_ir("suspend-defect", m);
    assert_eq!(
        (output.code, text(&output.stderr)),
        (DEFECT_EXIT_CODE, "defect: after\n")
    );

    // main stays suspended while a setTimeout callback panics. This is not `interrupted`.
    let mut m = emela_core::Module::new();
    let set_timeout = suspend::set_timeout(&mut m);
    let (never, wait) = suspend::never(&mut m);
    let callback = m.fn_type(vec![], TypeId::UNIT);
    let (h, x) = (
        m.local("h", Type::Handler(wait)),
        m.local("_", TypeId::UNIT),
    );
    suspend::main(
        &mut m,
        TypeId::UNIT,
        Expr::With {
            binds: vec![WithBind::new(h, call(never, vec![]))],
            body: Box::new(let_(
                x,
                call(
                    set_timeout,
                    vec![
                        lambda(
                            vec![],
                            callback,
                            builtin(Builtin::Panic, vec![string("late")]),
                        ),
                        int(0),
                    ],
                ),
                suspend::forever(wait, h),
            )),
        },
    );
    let output = run_ir("suspend-callback-defect", m);
    assert_eq!(
        (output.code, text(&output.stderr)),
        (DEFECT_EXIT_CODE, "defect: late\n")
    );
}

/// Files whose handler member `Open` is a `suspend handler`, and whose init does suspend: it
/// waits on the clock, which does not resume on the spot.
const SLOW_FILES: &str = r##"import Std.Clock.{Clock, SystemClock}
import Std.Console.{Console, SystemConsole}

effect ReadFile {
  fn read(max: Int) -> Int
}

effect Files {
  suspend handler Open(path: String): ReadFile
}

handler SlowFiles: Files {
  handler Open(path: String): ReadFile {
    init(path: String) {
      (use Clock).sleep(1)
      Self(path:)
    }

    fn read(max) { max * 2 }
  }
}
"##;

#[test]
fn effects_with_a_suspend_handler_member_may_suspend() {
    // An effect with a `suspend handler` member is not Immediate (`[type.effect-bound]`), so code
    // whose R has only that effect may suspend: a function with `uses { Files }`, and an init
    // whose R is `{ Files }` (issue #699).
    let cases = [
        (
            "suspend-member-function",
            r##"fn count() -> Int uses { Files } {
  with f = Files.Open("a") {
    f.read(1)
  }
}

fn main() {
  with SystemConsole, SystemClock, SlowFiles {
    (use Console).print_line("#{count()}")
  }
}
"##,
        ),
        (
            "suspend-member-init",
            r##"effect Holder {
  fn total() -> Int
}

handler Keeps(inner: use ReadFile): Holder {
  init {
    Self(inner: Files.Open("a"))
  }

  fn total() { self.inner.read(1) }
}

fn main() {
  with SystemConsole, SystemClock, SlowFiles, Keeps {
    (use Console).print_line("#{(use Holder).total()}")
  }
}
"##,
        ),
    ];
    for (name, main) in cases {
        let output = run_source(name, &format!("{SLOW_FILES}\n{main}"));
        assert_eq!(
            (output.code, text(&output.stdout), text(&output.stderr)),
            (0, "2\n", ""),
            "{name}"
        );
    }
}
