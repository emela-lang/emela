//! 並行性（仕様 11章）を `emela run` と `emela test` の経路で流す．ソースから lowering，codegen-js，
//! node までを通し，11章の例（11.2，11.3，11.5，11.7，11.8）の振る舞いを確かめる．
//!
//! 時刻は SystemClock の本物のタイマーで進める．待ち時間の差を十分に取り，起きる順だけを見る．

use std::path::{Path, PathBuf};

use emela_driver::{
    CoreJs, Input, Lower, LowerTests, MemoryFiles, Output, RunOptions, RunOutput, TestOptions,
    TestOutcome, run, test,
};

/// テストごとの出力先．前の実行の残りは消す．
fn out_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("emela-driver-fiber-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// `emela run` で main を走らせ，（終了コード，標準出力，標準エラー）を返す．
fn run_main(name: &str, main: &str) -> (i32, String, String) {
    let sources = [("app/Pome.toml", ""), ("app/src/main.emel", main)];
    let fs = MemoryFiles::new(sources.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = RunOptions {
        out_dir: out_dir(name),
        node: PathBuf::from("node"),
        args: Vec::new(),
        output: Output::Capture,
    };
    let (analysis, output) = run(
        &fs,
        &input,
        &mut Lower::default(),
        &mut CoreJs,
        &options,
        |_| {},
    );
    let _ = std::fs::remove_dir_all(&options.out_dir);
    assert!(
        analysis.diagnostics.is_empty(),
        "{:?}",
        analysis.diagnostics
    );
    let RunOutput {
        code,
        stdout,
        stderr,
    } = output.unwrap();
    (
        code,
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
    )
}

/// `emela test` で流し，テストごとの（名前，結果）を返す．
fn run_tests(name: &str, main: &str) -> Vec<(String, TestOutcome)> {
    let sources = [("app/Pome.toml", ""), ("app/src/main.emel", main)];
    let fs = MemoryFiles::new(sources.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = TestOptions {
        out_dir: out_dir(name),
        node: PathBuf::from("node"),
        filter: None,
        output: Output::Capture,
    };
    let mut out = Vec::new();
    let (analysis, report) = test(
        &fs,
        &input,
        &mut LowerTests::default(),
        &mut CoreJs,
        &options,
        |_| {},
        &mut out,
    );
    let _ = std::fs::remove_dir_all(&options.out_dir);
    assert_eq!(analysis.diagnostics, []);
    report
        .unwrap()
        .results
        .iter()
        .map(|r| (r.name.clone(), r.outcome.clone()))
        .collect()
}

/// どの例でも使う宣言．`fetch(n, ms)` は ms 待ってから `fetched n` を出して `n * 10` を返す．
/// `Res` は init と release を出すハンドラ．
const PRELUDE: &str = r##"import Std.Console.{Console, SystemConsole}
import Std.Clock.{Clock, SystemClock}
import Std.Fiber.{Fiber, Scope, Exit, Success, Failure}
import Std.Cause.{Cause, CauseKind, Fail, Die, Interrupt}
import Std.Deferred.{Deferred}

error Boom

effect Name {
  fn name() -> String
}

handler Outer implements Name {
  fn name() { "outer" }
}

handler Inner implements Name {
  fn name() { "inner" }
}

handler Res implements Name {
  init {
    (use Console).print_line("init res")
    Res
  }
  fn name() { "res" }
  release { (use Console).print_line("release res") }
}

fn say(text: String) -> () use { Console } {
  (use Console).print_line(text)
}

fn fetch(n: Int, ms: Int) -> Int use { Clock, Console } {
  (use Clock).sleep(ms)
  say("fetched #{n}")
  n * 10
}

fn boom() -> Int fails Boom { fail Boom }

fn kind_name(k: CauseKind) -> String {
  match k {
    Fail -> "Fail"
    Die -> "Die"
    Interrupt -> "Interrupt"
  }
}

fn show[A, E](e: Exit[A, E]) -> String {
  match e {
    Success(_) -> "Success"
    Failure(c) -> "#{kind_name(Cause.kind(c))} #{Cause.to_string(c)}"
  }
}
"##;

fn source(main: &str) -> String {
    format!("{PRELUDE}\n{main}")
}

#[test]
fn children_run_concurrently_and_are_joined() {
    // 11.2: 子は待ちを重ねて並行に走り，join で値を受ける．能力の値の fork と Fiber.fork のどちらも使える．
    let (code, out, err) = run_main(
        "scope",
        &source(
            r##"
fn main() {
  with SystemConsole, SystemClock {
    total = Fiber.scope(fn() {
      s = use Scope
      a = s.fork(fn() { fetch(1, 60) })
      b = Fiber.fork(fn() { fetch(2, 10) })
      Fiber.join(a) + Fiber.join(b)
    })
    say("total #{total}")
  }
}
"##,
        ),
    );
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (0, "fetched 2\nfetched 1\ntotal 30\n", "")
    );
}

#[test]
fn combinators() {
    // 11.3: all，race，both，map，timeout．結果は元の順で，race は最初に終わったものを採る．
    let (code, out, err) = run_main(
        "combinators",
        &source(
            r##"
fn main() {
  with SystemConsole, SystemClock {
    xs = Fiber.all([fn() { fetch(1, 60) }, fn() { fetch(2, 10) }])
    say("all #{xs}")
    w = Fiber.race([fn() { fetch(3, 200) }, fn() { fetch(4, 10) }])
    say("race #{w}")
    (p, q) = Fiber.both(fn() { fetch(5, 10) }, fn() { "x" })
    say("both #{p} #{q}")
    ys = Fiber.map([1, 2, 3], fn(i) { fetch(i, 100 - i * 40) })
    say("map #{ys}")
    t = Fiber.timeout(200, fn() { fetch(6, 10) })
    u = Fiber.timeout(10, fn() { fetch(7, 1000) })
    say("timeout #{t} #{u}")
    r = Fiber.race([fn() { boom() }, fn() { fetch(8, 1000) }]) escape { Boom -> -1 }
    say("race failed #{r}")
  }
}
"##,
        ),
    );
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (
            0,
            "fetched 2\nfetched 1\nall [10, 20]\nfetched 4\nrace 40\nfetched 5\nboth 50 x\n\
             fetched 3\nfetched 2\nfetched 1\nmap [10, 20, 30]\nfetched 6\n\
             timeout Some(60) None\nrace failed -1\n",
            ""
        )
    );
}

#[test]
fn children_get_the_handlers_where_scope_is_called() {
    // 11.2: 子が使うハンドラは Fiber.scope を呼んだ時点のもの．スコープの本体の中の with は子に
    // 届かない．ただし with の中で作ったクロージャは，その with のハンドラを使う．
    let (code, out, err) = run_main(
        "handlers",
        &source(
            r##"
fn main() {
  with SystemConsole, Outer {
    Fiber.scope(fn() {
      outside = fn() { say((use Name).name()) }
      with Inner {
        a = Fiber.fork(outside)
        b = Fiber.fork(fn() { say((use Name).name()) })
        say("body #{(use Name).name()}")
        Fiber.join(a)
        Fiber.join(b)
      }
    })
  }
}
"##,
        ),
    );
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (0, "body inner\nouter\ninner\n", "")
    );
}

#[test]
fn children_fork_siblings_into_their_own_scope() {
    // 11.2: 子の中の Scope はその子が属すスコープ自身．子は兄弟を fork でき，スコープはそれも待つ．
    // 入れ子のスコープでは，内側の子の fork は内側のスコープに入り，外側で取り出した能力の値は外側の
    // スコープへ fork する．内側の子の孫のエラーは内側のスコープに届く．
    let (code, out, err) = run_main(
        "siblings",
        &source(
            r##"
fn main() {
  with SystemConsole, SystemClock {
    Fiber.scope(fn() {
      a = Fiber.fork(fn() {
        b = Fiber.fork(fn() { fetch(2, 10) })
        say("child")
        Fiber.join(b)
      })
      say("joined #{Fiber.join(a)}")
    })
    r = Fiber.scope(fn() {
      outer = use Scope
      late = outer.fork(fn() { fetch(9, 50) })
      inner = Fiber.scope(fn() {
        Fiber.fork(fn() {
          h = Fiber.fork(fn() { boom() })
          Fiber.join(h)
        })
        (use Clock).sleep(1000)
        0
      }) escape { Boom -> -1 }
      say("inner #{inner}")
      Fiber.join(late)
    })
    say("outer #{r}")
  }
}
"##,
        ),
    );
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (
            0,
            "child\nfetched 2\njoined 20\ninner -1\nfetched 9\nouter 90\n",
            ""
        )
    );
}

#[test]
fn a_failing_child_interrupts_its_siblings() {
    // 11.2，11.4: 子の失敗は兄弟と本体を中断し，release を走らせてからスコープの失敗になる．
    // 本体が値を返したときも，残った子は中断される．
    let (code, out, err) = run_main(
        "child-fails",
        &source(
            r##"
fn main() {
  with SystemConsole, SystemClock {
    r = Fiber.scope(fn() {
      Fiber.fork(fn() { with Res { (use Clock).sleep(1000) } })
      Fiber.fork(fn() {
        (use Clock).sleep(10)
        boom()
      })
      (use Clock).sleep(1000)
      say("not reached")
      1
    }) escape { Boom -> -1 }
    say("scope #{r}")
    v = Fiber.scope(fn() {
      Fiber.fork(fn() { with Res { (use Clock).sleep(1000) } })
      (use Clock).sleep(10)
      2
    })
    say("value #{v}")
  }
}
"##,
        ),
    );
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (
            0,
            "init res\nrelease res\nscope -1\ninit res\nrelease res\nvalue 2\n",
            ""
        )
    );
}

#[test]
fn later_failures_are_notes() {
    // 11.2: 2つ以上の子が失敗したら，先の失敗が結果で，後の失敗は注記になる．
    let (code, out, err) = run_main(
        "notes",
        &source(
            r##"
fn main() {
  with SystemConsole, SystemClock {
    Fiber.scope(fn() {
      Fiber.fork(fn() {
        (use Clock).sleep(10)
        panic("first")
      })
      Fiber.fork(fn() {
        with Res {
          (use Clock).sleep(1000)
        }
      })
      (use Clock).sleep(1000)
    })
  }
}
"##,
        )
        .replace(
            "release { (use Console).print_line(\"release res\") }",
            "release { panic(\"second\") }",
        ),
    );
    assert_eq!((code, out.as_str()), (101, "init res\n"), "{err}");
    assert!(err.contains("first"), "{err}");
    assert!(
        err.contains("note: another fiber also failed: defect: second"),
        "{err}"
    );
}

#[test]
fn attempt_and_await() {
    // 11.5 の表: attempt は Fail と Die を受け，await は中断を受ける．中断された子の join は defect．
    // Cause.raise で起こし直したエラーは escape で受けられる．
    let (code, out, err) = run_main(
        "attempt-await",
        &source(
            r##"
fn main() {
  with SystemConsole, SystemClock {
    say(show(Fiber.attempt(fn() { boom() })))
    say(show(Fiber.attempt(fn() { panic("bad") })))
    say(show(Fiber.attempt(fn() { fetch(1, 1) })))
    Fiber.scope(fn() {
      h = Fiber.fork(fn() { with Res { (use Clock).sleep(1000) } })
      (use Clock).sleep(10)
      Fiber.interrupt(h)
      say("await #{show(Fiber.await(h))}")
      say("join #{show(Fiber.attempt(fn() { Fiber.join(h) }))}")
      ok = Fiber.fork(fn() { 7 })
      say("await #{show(Fiber.await(ok))}")
    })
    r = Fiber.scope(fn() {
      match Fiber.attempt(fn() { boom() }) {
        Success(v) -> v
        Failure(c) -> Cause.raise(c)
      }
    }) escape { Boom -> -2 }
    say("raise #{r}")
  }
}
"##,
        ),
    );
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (
            0,
            "Fail error: Boom\nDie defect: bad\nfetched 1\nSuccess\ninit res\nrelease res\n\
             await Interrupt interrupted\njoin Die defect: fiber was interrupted\n\
             await Success\nraise -2\n",
            ""
        )
    );
}

#[test]
fn deferred() {
    // 11.7: Deferred は一度だけ値を入れられ，待っている全員が再開する．poll は待たない．
    let (code, out, err) = run_main(
        "deferred",
        &source(
            r##"
fn main() {
  with SystemConsole, SystemClock {
    Fiber.scope(fn() {
      d = Fiber.deferred()
      a = Fiber.fork(fn() { Deferred.await(d) + 1 })
      b = Fiber.fork(fn() { Deferred.await(d) + 2 })
      Fiber.fork(fn() {
        (use Clock).sleep(10)
        Deferred.complete(d, 40)
      })
      say("poll #{Deferred.poll(d)}")
      say("joined #{Fiber.join(a)} #{Fiber.join(b)}")
      say("again #{Deferred.complete(d, 1)} #{Deferred.poll(d)}")
    })
  }
}
"##,
        ),
    );
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (0, "poll None\njoined 41 42\nagain False Some(40)\n", "")
    );
}

#[test]
fn using_an_ended_scope_is_a_defect() {
    // 11.8: 型で止まらない持ち出し（クロージャで捕まえて外へ出す）は実行時に defect になる．
    let (code, out, err) = run_main(
        "ended-scope",
        &source(
            r##"
fn main() {
  with SystemConsole {
    join_later = Fiber.scope(fn() {
      h = Fiber.fork(fn() { 1 })
      fn() { Fiber.join(h) }
    })
    say(show(Fiber.attempt(fn() { join_later() })))
    complete_later = Fiber.scope(fn() {
      d = Fiber.deferred()
      fn() { Deferred.complete(d, 1) }
    })
    say(show(Fiber.attempt(fn() { complete_later() })))
    poll_later = Fiber.scope(fn() {
      d = Fiber.deferred()
      fn() { Deferred.poll(d) }
    })
    poll_later()
    ()
  }
}
"##,
        ),
    );
    assert_eq!(
        (code, out.as_str()),
        (
            101,
            "Die defect: fiber was joined after its scope ended\n\
             Die defect: deferred was completed after its scope ended\n"
        )
    );
    assert!(
        err.contains("deferred was polled after its scope ended"),
        "{err}"
    );
}

#[test]
fn fibers_in_tests() {
    // 13章: テスト関数の中でも fiber が動く．子の失敗はそのテストのエラーか defect になる．
    let outcomes = run_tests(
        "tests",
        &source(
            r##"
@test
fn joins() {
  with SystemConsole, SystemClock {
    xs = Fiber.all([fn() { fetch(1, 20) }, fn() { fetch(2, 10) }])
    assert xs == [10, 20]
  }
}

@test
fn races() {
  with SystemConsole, SystemClock {
    assert Fiber.race([fn() { fetch(1, 200) }, fn() { fetch(2, 10) }]) == 20
  }
}

@test
fn child_error() {
  Fiber.scope(fn() {
    h = Fiber.fork(fn() { boom() })
    Fiber.join(h)
  })
}

@test
fn child_defect() {
  Fiber.scope(fn() {
    h = Fiber.fork(fn() { panic("child") })
    Fiber.join(h)
  })
}
"##,
        ),
    );
    assert_eq!(
        outcomes,
        [
            ("joins".to_owned(), TestOutcome::Passed),
            ("races".to_owned(), TestOutcome::Passed),
            ("child_error".to_owned(), TestOutcome::Error("Boom".into())),
            (
                "child_defect".to_owned(),
                TestOutcome::Defect("child".into())
            ),
        ]
    );
}

#[test]
fn test_clock_runs_on_virtual_time() {
    // 8.8: TestClock は仮の時刻で動く．どの fiber も進めなくなったときに一番早い予定の時刻まで進め，
    // 同じ時刻なら sleep を呼んだ順に起こす．1時間の sleep を含む Fiber.timeout と race も
    // 本当には待たずに終わる（11.3）．
    let started = std::time::Instant::now();
    let outcomes = run_tests(
        "virtual-time",
        &format!(
            "{}{PRELUDE}{}",
            "import Std.Clock.{TestClock}\nimport Std.Console.{TestConsole}\n\
             import Std.State.{State}\nimport Std.Instant.{Instant}\n",
            r##"
fn at(text: String) -> () use { Clock, Console } {
  ms = Instant.to_unix_milliseconds((use Clock).now())
  say("#{text} at #{ms}")
}

fn zero() -> Instant {
  Instant.from_unix_milliseconds(Int64.from_int(0))
}

@test
fn wakes_in_time_order() {
  ((_, lines), end) = State.run(zero(), fn() {
    State.run([], fn() {
      with TestConsole, TestClock {
        Fiber.scope(fn() {
          Fiber.fork(fn() {
            (use Clock).sleep(30)
            at("a")
          })
          Fiber.fork(fn() {
            (use Clock).sleep(10)
            at("b")
          })
          Fiber.fork(fn() {
            (use Clock).sleep(10)
            at("c")
          })
          (use Clock).sleep(40)
          at("body")
        })
      }
    })
  })
  assert lines == ["b at 10", "c at 10", "a at 30", "body at 40"]
  assert Instant.to_unix_milliseconds(end) == Int64.from_int(40)
}

@test
fn timeout_gives_up_at_once() {
  (r, end) = State.run(zero(), fn() {
    with TestClock {
      Fiber.timeout(60000, fn() {
        (use Clock).sleep(3600000)
        1
      })
    }
  })
  assert r == None
  assert Instant.to_unix_milliseconds(end) == Int64.from_int(60000)
}

@test
fn timeout_returns_in_time() {
  (r, end) = State.run(zero(), fn() {
    with TestClock {
      Fiber.timeout(60000, fn() {
        (use Clock).sleep(10)
        5
      })
    }
  })
  assert r == Some(5)
  assert Instant.to_unix_milliseconds(end) == Int64.from_int(10)
}

@test
fn race_takes_the_earliest() {
  (r, end) = State.run(zero(), fn() {
    with TestClock {
      Fiber.race([
        fn() {
          (use Clock).sleep(3600000)
          1
        },
        fn() {
          (use Clock).sleep(10)
          2
        },
      ])
    }
  })
  assert r == 2
  assert Instant.to_unix_milliseconds(end) == Int64.from_int(10)
}
"##,
        ),
    );
    assert_eq!(
        outcomes,
        [
            ("wakes_in_time_order".to_owned(), TestOutcome::Passed),
            ("timeout_gives_up_at_once".to_owned(), TestOutcome::Passed),
            ("timeout_returns_in_time".to_owned(), TestOutcome::Passed),
            ("race_takes_the_earliest".to_owned(), TestOutcome::Passed),
        ]
    );
    assert!(
        started.elapsed().as_secs() < 30,
        "took {:?}",
        started.elapsed()
    );
}
