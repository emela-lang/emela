//! suspend と Async（仕様 8.4，8.7，14.5）を，`emela run` と `emela test` の経路で動かす結合テスト．
//!
//! 仕様 8.4 の JsClock の形（`setTimeout` で後から resume する `suspend fn sleep`）を，ソースの隣の
//! `.mjs` で書く．その場で resume する InstantClock も同じプログラムで使う．

use std::path::PathBuf;
use std::process::Command;

struct Project {
    dir: PathBuf,
}

impl Project {
    fn new(name: &str, main: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("emela-cli-suspend-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let files = [
            ("Pome.toml", ""),
            ("src/clock.mjs", CLOCK_MJS),
            ("src/main.emel", &format!("{CLOCK}\n{main}")),
        ];
        for (path, text) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Project { dir }
    }

    fn emela(&self, args: &[&str]) -> (Option<i32>, String, String) {
        let output = Command::new(env!("CARGO_BIN_EXE_emela"))
            .args(args)
            .current_dir(&self.dir)
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        (
            output.status.code(),
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap(),
        )
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const CLOCK_MJS: &str = "export function after(ms, f) { setTimeout(f, ms); }\n";

/// 仕様 8.4 の JsClock と，その場で resume する InstantClock．
const CLOCK: &str = r#"effect Clock {
  suspend fn sleep(ms: Int) -> Int
}

@external(js, "./clock.mjs", "after")
fn after(ms: Int, f: fn() -> ()) -> ()

handler JsClock implements Clock {
  suspend fn sleep(ms, resume) {
    after(ms, fn() { resume(ms) })
  }
}

handler InstantClock implements Clock {
  suspend fn sleep(ms, resume) { resume(0) }
}
"#;

#[test]
fn clocks_resume_later_and_in_place() {
    // JsClock の sleep は setTimeout の後で再開する．InstantClock は中断しない．
    // 中断しうるラムダを渡した List.map は中断用の写しを，渡さない List.map は元の関数を呼ぶ．
    let project = Project::new(
        "clocks",
        r#"fn total(xs: List[Int]) -> Int use Clock {
  clock = use Clock
  List.fold(List.map(xs, fn(ms) { clock.sleep(ms) }), 0, fn(a, b) { a + b })
}

fn main() {
  with JsConsole {
    console = use Console
    with JsClock {
      console.print_line("start")
      slept = (use Clock).sleep(20)
      console.print_line("slept #{slept}")
      console.print_line("total #{total([3, 1, 2])}")
    }
    with InstantClock {
      console.print_line("instant #{total([5, 5])}")
    }
    console.print_line("doubled #{List.map([1, 2], fn(n) { n * 2 })}")
  }
}
"#,
    );
    let (code, stdout, stderr) = project.emela(&["run"]);
    assert_eq!(
        (code, stdout.as_str(), stderr.as_str()),
        (
            Some(0),
            "start\nslept 20\ntotal 6\ninstant 0\ndoubled [2, 4]\n",
            ""
        )
    );
    let js = std::fs::read_to_string(project.dir.join("target/emela/js/main.mjs")).unwrap();
    assert!(js.contains("function* main() {"), "{js}");
    assert!(js.contains("function* List_map$suspend"), "{js}");
    assert!(js.contains("function List_map("), "{js}");
}

#[test]
fn resuming_twice_is_a_defect() {
    let project = Project::new(
        "twice",
        r#"handler TwiceClock implements Clock {
  suspend fn sleep(ms, resume) {
    resume(1)
    resume(2)
  }
}

fn main() {
  with TwiceClock { (use Clock).sleep(1) }
}
"#,
    );
    let (code, stdout, stderr) = project.emela(&["run"]);
    assert_eq!(
        (code, stdout.as_str(), stderr.as_str()),
        (
            Some(101),
            "",
            "defect: resume called twice in operation `sleep` of handler `TwiceClock`\n"
        )
    );
}

#[test]
fn dropped_resume_interrupts_main() {
    let project = Project::new(
        "dropped",
        r#"handler NeverClock implements Clock {
  suspend fn sleep(ms, resume) { () }
}

fn main() {
  with JsConsole {
    console = use Console
    with NeverClock {
      console.print_line("before")
      (use Clock).sleep(1)
      console.print_line("never")
    }
  }
}
"#,
    );
    let (code, stdout, stderr) = project.emela(&["run"]);
    assert_eq!(
        (code, stdout.as_str(), stderr.as_str()),
        (Some(130), "before\n", "interrupted\n")
    );
}

#[test]
fn tests_wait_for_clocks() {
    // テスト関数の R の Async はテストランナーが供給する（13.3）．resume されないテストは失敗．
    let project = Project::new(
        "test",
        r#"handler NeverClock implements Clock {
  suspend fn sleep(ms, resume) { () }
}

@test
fn sleeps() {
  with JsClock { assert (use Clock).sleep(5) == 5 }
}

@test
fn instant() {
  with InstantClock { assert (use Clock).sleep(5) == 0 }
}

@test
fn never_resumed() {
  with NeverClock { (use Clock).sleep(1) }
}
"#,
    );
    let (code, stdout, stderr) = project.emela(&["test"]);
    assert_eq!(code, Some(1), "{stdout}{stderr}");
    assert!(
        stdout.contains("test sleeps ... ok\ntest instant ... ok\ntest never_resumed ... FAILED\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("---- never_resumed ----\ninterrupted\n"),
        "{stdout}"
    );
}
