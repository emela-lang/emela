//! The bundled handlers of the standard library through the `emela run` path: random numbers,
//! the environment, the clock, state, the platform layer and HTTP.

use std::path::PathBuf;

use emela_driver::Lower;

use super::{out_dir, run_source, run_source_in, text};

#[test]
fn random_runs_through_handlers() {
    // The bundled Random is provided by its System handler (spec 15.2). Values differ each run, so
    // only the range is checked.
    let output = run_source(
        "random",
        r##"import Std.Console.{Console, SystemConsole}
import Std.Random.{Random, SystemRandom}

fn draw(n: Int) {
  if n > 0 {
    random = use Random
    console = use Console
    console.print_line("#{random.int()} #{random.float()}")
    draw(n - 1)
  }
}

fn main() {
  with SystemConsole, SystemRandom {
    draw(100)
  }
}
"##,
    );
    assert_eq!((output.code, text(&output.stderr)), (0, ""));
    let mut ints = Vec::new();
    for line in text(&output.stdout).lines() {
        let (int, float) = line.split_once(' ').unwrap();
        ints.push(int.parse::<i32>().unwrap());
        let float: f64 = float.parse().unwrap();
        assert!((0.0..1.0).contains(&float), "{float} is not in [0, 1)");
    }
    assert_eq!(ints.len(), 100);
    ints.sort();
    ints.dedup();
    assert!(ints.len() > 1, "all 100 values are the same");
}

/// Writes a script that sets environment variables and then starts node, returning its path.
/// Calling `std::env::set_var` inside a test would race with parallel tests, so the variables
/// are passed to node only.
#[cfg(unix)]
fn node_with_env(name: &str, exports: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let dir = out_dir(&format!("{name}-node"));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("node");
    std::fs::write(&path, format!("#!/bin/sh\n{exports}\nexec node \"$@\"\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn env_runs_through_handlers() {
    // The bundled Env is provided by its System handler (spec 15.2). An empty value is
    // distinguished from an unset one. The second half reads other values just by swapping the
    // handler (dependency injection, spec chapter 13).
    let node = node_with_env(
        "env",
        "export EMELA_TEST_SET=hello EMELA_TEST_EMPTY=\nunset EMELA_TEST_UNSET",
    );
    let output = run_source_in(
        "env",
        r##"import Std.Console.{Console, SystemConsole}
import Std.Env.{Env, SystemEnv}

fn show(name: String) -> String {
  (use Env).get(name) match {
    Some(value) -> "#{name}=#{value}"
    None -> "#{name} is unset"
  }
}

handler FixedEnv: Env {
  fn get(name) { Some("fixed") }
}

fn main() {
  with SystemConsole, SystemEnv {
    console = use Console
    console.print_line(show("EMELA_TEST_SET"))
    console.print_line(show("EMELA_TEST_EMPTY"))
    console.print_line(show("EMELA_TEST_UNSET"))
  }
  with SystemConsole, FixedEnv {
    (use Console).print_line(show("EMELA_TEST_UNSET"))
  }
}
"##,
        &mut Lower::default(),
        node.clone(),
    );
    let _ = std::fs::remove_dir_all(node.parent().unwrap());
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (
            0,
            "EMELA_TEST_SET=hello\nEMELA_TEST_EMPTY=\nEMELA_TEST_UNSET is unset\nEMELA_TEST_UNSET=fixed\n",
            ""
        )
    );
}

#[test]
fn clock_runs_through_js_clock() {
    // The bundled Clock is provided by SystemClock (`Date.now()`, the bundled `.mjs`) (spec 8.1,
    // 15.2). The time differs each run, so only direction and range are checked.
    let output = run_source(
        "clock",
        r#"import Std.Console.{Console, SystemConsole}
import Std.Clock.{Clock, SystemClock}
import Std.Instant.{Instant}
import Std.Duration.{Duration}

fn elapsed(start: Instant) -> Duration uses { Clock } {
  (use Clock).now() |> Instant.since(start)
}

fn main() {
  with SystemConsole, SystemClock {
    c = use Console
    start = (use Clock).now()
    d = elapsed(start)
    c.print_line(if Instant.to_unix_milliseconds(start) > 1700000000000 { "after 2023" } else { "before" })
    c.print_line(if Duration.to_milliseconds(d) >= 0 { "forward" } else { "backward" })
    c.print_line(if String.ends_with(Instant.to_string(start), "Z") { "iso" } else { "other" })
  }
}
"#,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "after 2023\nforward\niso\n", "")
    );
}

#[test]
fn clock_sleeps_through_js_clock() {
    // SystemClock's sleep resumes after setTimeout (spec 8.4, 8.7), and main waits to be resumed
    // before finishing. It also waits inside List.map given a suspending function (the suspending
    // copy).
    let output = run_source(
        "clock-sleep",
        r##"import Std.Console.{Console, SystemConsole}
import Std.Clock.{Clock, SystemClock}
import Std.Instant.{Instant}
import Std.Duration.{Duration}

fn nap(clock_name: String, ms: Int) -> String uses { Clock } {
  clock = use Clock
  start = clock.now()
  clock.sleep(ms)
  waited = Duration.to_milliseconds(clock.now() |> Instant.since(start))
  if waited >= Int64.from_int(ms - 5) { "#{clock_name} waited" } else { "#{clock_name} did not wait" }
}

fn main() {
  with SystemConsole, SystemClock {
    c = use Console
    c.print_line(nap("js", 50))
    List.map([30, 10, 20], |ms| nap("js #{ms}", ms)) |> List.map(c.print_line)
    c.print_line("done")
  }
}
"##,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (
            0,
            "js waited\njs 30 waited\njs 10 waited\njs 20 waited\ndone\n",
            ""
        )
    );
}

#[test]
fn system_clock_sleeps_zero_for_a_negative_ms() {
    // A negative ms counts as 0 (spec 8.8). SystemClock clamps before passing it to the host, so
    // Node 24's TimeoutNegativeWarning does not appear on stderr.
    let output = run_source(
        "clock-negative-sleep",
        r##"import Std.Console.{Console, SystemConsole}
import Std.Clock.{Clock, SystemClock}

fn main() {
  with SystemConsole, SystemClock {
    (use Clock).sleep(-5)
    (use Console).print_line("woke")
  }
}
"##,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "woke\n", "")
    );
}

#[test]
fn state_run_returns_value_and_final_state() {
    // State.run (spec 8.8) returns the body's value paired with the final state. States with
    // different type arguments are different effects, and a generic fn (`modify[S]`) picks the
    // caller's State[Int] or State[String] (spec 8.1). If the body fails, the state is discarded
    // and only the error propagates. The function passed to update runs in one step.
    let output = run_source(
        "state-run",
        r##"import Std.Console.{Console, SystemConsole}
import Std.State.{State}

error Boom

fn count() -> Int uses { State[Int] } {
  st = use State[Int]
  st.update(|n| n + 1)
  st.get()
}

fn modify[S](f: fn(S) -> S fails {} uses {}) -> S uses { State[S] } {
  (use State[S]).update(f)
  (use State[S]).get()
}

fn twice[T](f: fn(T) -> T fails {} uses {}) -> T uses { State[T] } {
  modify(f)
  modify(f)
}

fn both() -> String uses { State[Int], State[String] } {
  n = modify(|n| n + 1)
  (use State[String]).set("n=#{n}")
  twice(|s| String.append(s, "!"))
}

fn peek() { modify(|x| x) }

fn failing() -> Int fails Boom {
  (_, last) = State.run(1, || {
    (use State[Int]).set(5)
    fail Boom
  })
  last
}

fn main() {
  with SystemConsole {
    c = use Console
    (n, last) = State.run(0, || count() + count())
    c.print_line("#{n} #{last}")
    ((s, inner), outer) = State.run(10, || State.run("", || both()))
    c.print_line("#{s} #{inner} #{outer}")
    (v, same) = State.run(7, || peek())
    c.print_line("#{v} #{same}")
    f = modify
    (g, h) = State.run(3, || f(|k| k * 4))
    c.print_line("#{g} #{h}")
    c.print_line("#{failing() escape { Boom -> -1 }}")
  }
}
"##,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "3 2\nn=11!! n=11!! 11\n7 7\n12 12\n-1\n", "")
    );
}

#[test]
fn test_handlers_record_output_and_move_the_clock() {
    // The bundled TestConsole collects output in State[List[String]] in write order, and TestClock
    // runs on virtual time in State[Instant] (spec 8.8, 15.2). With one fiber, the result is the
    // same as advancing time by each sleep and continuing.
    let output = run_source(
        "state-test-handlers",
        r##"import Std.Console.{Console, SystemConsole, TestConsole}
import Std.Clock.{Clock, TestClock}
import Std.State.{State}
import Std.Instant.{Instant}
import Std.Duration.{Duration}

fn greet(name: String) -> () uses { Console } {
  (use Console).print_line("Hello, #{name}!")
  (use Console).print_error_line("dropped")
}

fn elapsed() -> Duration uses { Clock } {
  clock = use Clock
  start = clock.now()
  clock.sleep(1500)
  clock.sleep(-3)
  clock.now() |> Instant.since(start)
}

fn main() {
  (_, lines) = State.run([], || {
    with TestConsole {
      greet("alice")
      greet("bob")
    }
  })
  start = Instant.from_unix_milliseconds(Int64.from_int(1000))
  (d, end) = State.run(start, || with TestClock { elapsed() })
  with SystemConsole {
    c = use Console
    c.print_line(String.join(lines, "|"))
    c.print_line("#{Duration.to_milliseconds(d)} #{Instant.to_unix_milliseconds(end)}")
  }
}
"##,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "Hello, alice!|Hello, bob!\n1500 2500\n", "")
    );
}

#[test]
fn state_run_keeps_the_state_across_suspension() {
    // Even if the body suspends on SystemClock (spec 8.7, 14.5), State.run's state carries over
    // after resuming.
    let output = run_source(
        "state-suspend",
        r##"import Std.Console.{Console, SystemConsole}
import Std.Clock.{Clock, SystemClock}
import Std.State.{State}

fn tick() -> () uses { State[Int], Clock } {
  (use Clock).sleep(10)
  (use State[Int]).update(|n| n + 1)
}

fn main() {
  (_, n) = State.run(0, || {
    with SystemClock {
      tick()
      tick()
      List.map([1, 2, 3], |x| tick())
    }
  })
  with SystemConsole { (use Console).print_line("#{n}") }
}
"##,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "5\n", "")
    );
}

#[test]
fn effects_differing_only_in_a_capability_type_are_different_effects() {
    // `State[use Name]` and `State[use Count]` are two effects (`[effect.use]`): the inner
    // `State.run` does not take the place of the outer one, nested or one after the other
    // (issue #705).
    let output = run_source(
        "state-of-capabilities",
        r##"import Std.Console.{Console, SystemConsole}
import Std.State.{State}

effect Name {
  fn name() -> String
}

effect Count {
  fn count() -> Int
}

handler Alice: Name {
  fn name() { "alice" }
}

handler Three: Count {
  fn count() { 3 }
}

fn main() {
  with SystemConsole, Alice, Three {
    State.run(use Name, || {
      State.run(use Count, || {
        n = (use State[use Name]).get()
        c = (use State[use Count]).get()
        (use Console).print_line("#{n.name()} #{c.count()}")
      })
    })
    State.run(use Name, || (use Console).print_line((use State[use Name]).get().name()))
    State.run(use Count, || {
      c = (use State[use Count]).get()
      (use Console).print_line("#{c.count()}")
    })
    ()
  }
}
"##,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "alice 3\nalice\n3\n", "")
    );
}

#[test]
fn effects_differing_in_the_type_arguments_of_a_capability_type_are_different_effects() {
    // `State[use Box[Int]]` and `State[use Box[String]]` are two effects: the type arguments of
    // the effect in a capability type are in the key (`[effect.use]`, issue #705), also when the
    // key is looked up in another function.
    let output = run_source(
        "state-of-generic-capabilities",
        r##"import Std.Console.{Console, SystemConsole}
import Std.State.{State}

effect Box[T] {
  fn get() -> T
}

handler Seven: Box[Int] {
  fn get() { 7 }
}

handler Word: Box[String] {
  fn get() { "word" }
}

fn word() -> String uses { State[use Box[String]], Box[String] } {
  (use State[use Box[String]]).get().get()
}

fn main() {
  with SystemConsole, Seven, Word {
    State.run(use Box[Int], || {
      State.run(use Box[String], || {
        n = (use State[use Box[Int]]).get().get()
        (use Console).print_line("#{n} #{word()}")
      })
    })
    State.run(use Box[String], || (use Console).print_line(word()))
    ()
  }
}
"##,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "7 word\nword\n", "")
    );
}

#[test]
fn platform_layer_provides_system_handlers() {
    // `Std.Platform.Platform` is a layer bundling the System handlers (spec 15.2). Some can be
    // replaced by Test handlers listed after it (spec 8.3). std modules are used through imports.
    let output = run_source(
        "platform",
        r##"import Std.Platform.{Platform}
import Std.Console.{Console, TestConsole}
import Std.Clock.{Clock, TestClock}
import Std.Instant
import Std.State.{State}

fn main() {
  with Platform {
    now = Instant.to_unix_milliseconds((use Clock).now())
    (use Console).print_line("now > 0: #{now > Int64.from_int(0)}")
  }
  start = Instant.from_unix_milliseconds(Int64.from_int(5))
  (_, end) = State.run(start, || with Platform, TestClock { (use Clock).sleep(10) })
  (_, lines) = State.run([], || with Platform, TestConsole { (use Console).print_line("x") })
  with Platform {
    (use Console).print_line("#{Instant.to_unix_milliseconds(end)} #{lines}")
  }
}
"##,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (0, "now > 0: True\n15 [\"x\"]\n", "")
    );
}

#[test]
fn parse_and_conversions() {
    // The spec 15.3 API: parse fails with ParseError on malformed input; String.concat and
    // List.flatten flatten; String.append joins two; conversions live on the depending side
    // (Float.from_int, Int64.from_int).
    let output = run_source(
        "parse",
        r##"import Std.Console.{Console, SystemConsole}

fn int(s: String) -> String {
  Int.to_string(Int.parse(s)) escape { ParseError(input:) -> "bad #{input}" }
}

fn float(s: String) -> String {
  Float.to_string(Float.parse(s)) escape { ParseError(input:) -> "bad #{input}" }
}

fn main() {
  with SystemConsole {
    c = use Console
    c.print_line(String.join(List.map(["42", "-7", "+3", "2147483647", "-2147483648"], int), " "))
    c.print_line(String.join(List.map(["", " 1", "1.0", "2147483648", "0x10", "-"], int), "|"))
    c.print_line(String.join(List.map(["1.5", "-0.25", "3", "1e+21", "Infinity", "NaN"], float), " "))
    c.print_line(String.join(List.map(["", "1.", ".5", "1e", "inf"], float), "|"))
    c.print_line(String.concat(["a", "b", "c"]))
    c.print_line(String.append("ab", "cd"))
    c.print_line("#{List.flatten([[1], [], [2, 3]])}")
    c.print_line("#{Float.from_int(7)} #{Int64.from_int(-1)}")
    c.print_line("#{Option.or(None, 5)} #{List.fold([1, 2, 3], from: 10, f: |a, b| a + b)}")
  }
}
"##,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (
            0,
            "42 -7 3 2147483647 -2147483648\n\
             bad |bad  1|bad 1.0|bad 2147483648|bad 0x10|bad -\n\
             1.5 -0.25 3.0 1e+21 Infinity NaN\n\
             bad |bad 1.|bad .5|bad 1e|bad inf\n\
             abc\nabcd\n[1, 2, 3]\n7.0 -1\n5 16\n",
            ""
        )
    );
}

#[test]
fn test_http_answers_from_the_test_server() {
    // The bundled TestHttp decides responses by passing the request and its attempt number (from
    // 0) to the respond of State[TestServer]; `None` becomes ConnectionFailed. Sent requests are
    // appended to sent in order (spec 15.2). Functions above send (get, post, header, headers,
    // require_ok, encode_query) are plain value functions callable without a handler. The names
    // that moved to `Std.Http.Request` and `Std.Http.Response` stay in `Std.Http` (`[api.std.http]`).
    let output = run_source(
        "http-test",
        r##"import Std.Console.{Console, SystemConsole}
import Std.Http.{Http, TestServer, TestHttp, ConnectionFailed}
import Std.Http.Request.{Request, Get, Post, Delete}
import Std.Http.Response.{Response, UnexpectedStatus}
import Std.State.{State}

fn reply(req: Request, n: Int) -> Option[Response] {
  if n >= 2 {
    None
  } else {
    Some(Response(status: 200 + n, headers: [("X-A", "1"), ("x-a", "2")], body: "#{n}:#{req.body}"))
  }
}

fn method(req: Request) -> String {
  req.method match {
    Get -> "GET"
    Post -> "POST"
    _ -> "other"
  }
}

fn main() {
  server = TestServer(respond: reply, sent: [])
  (out, after) = State.run(server, || {
    with TestHttp {
      a = Http.get("https://example.com/a")
      b = Http.post("https://example.com/b", "body")
      c = "#{Http.get("https://example.com/c").status}" escape {
        ConnectionFailed(url:, message:) -> "failed #{url}"
      }
      "#{a.status} #{a.body} | #{b.status} #{b.body} | #{c} | #{Response.header(a, "x-A")} #{Response.header_values(b, "X-a")} #{Response.header(a, "none")}"
    }
  })
  with SystemConsole {
    c = use Console
    c.print_line(out)
    c.print_line(String.join(List.map(after.sent, |req| "#{method(req)} #{req.url} #{req.body}"), ", "))
    c.print_line(Request.encode_query([("q", "東 京"), ("a&b", "c=d"), ("page", "2")]))
    c.print_line("#{Response.require_ok(Response.new(204, "")).status}")
    c.print_line("#{Response.require_ok(Response.new(404, "nf")).status}" escape {
      UnexpectedStatus(status:, body:) -> "unexpected #{status} #{body}"
    })
    r = Request.new(Delete, "u") |> Request.add_header("a", "1") |> Request.add_header("a", "2")
    c.print_line("#{r.headers} #{r.body == ""}")
    old = Http.request(Delete, "u") |> Http.add_header("a", "1")
    res = Http.require_ok(Http.response(200, "")) |> Response.add_header("B", "2")
    c.print_line("#{old.headers} #{Http.header(res, "b")} #{Http.headers(res, "b")} #{Http.encode_query([("a", " ")])}")
  }
}
"##,
    );
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (
            0,
            "200 0: | 201 1:body | failed https://example.com/c | Some(\"1\") [\"1\", \"2\"] None\n\
             GET https://example.com/a , POST https://example.com/b body, GET https://example.com/c \n\
             q=%E6%9D%B1%20%E4%BA%AC&a%26b=c%3Dd&page=2\n\
             204\n\
             unexpected 404 nf\n\
             [(\"a\", \"1\"), (\"a\", \"2\")] True\n\
             [(\"a\", \"1\")] Some(\"2\") [\"2\"] a=%20\n",
            ""
        )
    );
}

/// A test server on 127.0.0.1 using node's `http`. Stopped on drop.
struct LocalServer {
    child: std::process::Child,
    dir: PathBuf,
    port: u16,
    /// A port opened and closed immediately, used as a target that refuses connections.
    closed_port: u16,
}

impl LocalServer {
    fn start(name: &str) -> Self {
        use std::io::BufRead;

        let dir = out_dir(&format!("{name}-server"));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("server.mjs");
        std::fs::write(&script, LOCAL_SERVER).unwrap();
        let mut child = std::process::Command::new("node")
            .arg(&script)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let (port, closed_port) = line.trim().split_once(' ').unwrap();
        LocalServer {
            child,
            dir,
            port: port.parse().unwrap(),
            closed_port: closed_port.parse().unwrap(),
        }
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A server returning fixed responses per path. `/echo` returns the request's method,
/// content-type, x-one and body, and sets several headers with the same name. On start it writes
/// the listening port and the closed port on one line.
const LOCAL_SERVER: &str = r#"import http from "node:http";
const closed = http.createServer();
closed.listen(0, "127.0.0.1", () => {
  const closedPort = closed.address().port;
  closed.close(() => {
    const server = http.createServer((req, res) => {
      let body = "";
      req.setEncoding("utf8");
      req.on("data", (c) => (body += c));
      req.on("end", () => {
        switch (req.url) {
          case "/echo":
            res.setHeader("Set-Cookie", ["a=1", "b=2"]);
            res.setHeader("X-Multi", ["p", "q"]);
            res.setHeader("Content-Type", "text/plain; charset=utf-8");
            res.end(`${req.method} ${req.headers["content-type"] ?? "-"} ${req.headers["x-one"] ?? "-"} ${body}`);
            break;
          case "/binary":
            res.end(Buffer.from([0xff, 0xfe, 0x00]));
            break;
          case "/redirect":
            res.writeHead(302, { Location: "/echo" });
            res.end();
            break;
          case "/loop":
            res.writeHead(302, { Location: "/loop" });
            res.end();
            break;
          default:
            res.writeHead(404);
            res.end("not found 東");
        }
      });
    });
    server.listen(0, "127.0.0.1", () => console.log(`${server.address().port} ${closedPort}`));
  });
});
"#;

#[test]
fn system_http_talks_to_a_local_server() {
    // Points the bundled SystemHttp (fetch, spec 15.2) at the local server; nothing leaves the
    // machine. Response headers are lowercased and sorted by name, same-named headers are joined
    // with `, `, and set-cookie stays separate. date and others change every time, so only chosen
    // names are checked. 4xx is not a failure. Redirects are followed. Connection failure messages
    // vary across Node versions, so only the cause code on a refused connection is checked.
    let server = LocalServer::start("http-system");
    let base = format!("http://127.0.0.1:{}", server.port);
    let closed = format!("http://127.0.0.1:{}/", server.closed_port);
    let main = r##"import Std.Console.{Console}
import Std.Platform.{Platform}
import Std.Http.{Http, InvalidUrl, ConnectionFailed, ResponseNotText}
import Std.Http.Request.{Request, Get, Head, Post}

fn picked(headers: List[(String, String)]) -> List[(String, String)] {
  List.filter(headers, |pair| {
    (name, _) = pair
    name == "content-type" || name == "set-cookie" || name == "x-multi"
  })
}

fn show(req: Request) -> String fails InvalidUrl | ConnectionFailed | ResponseNotText uses { Http } {
  res = (use Http).send(req)
  "#{res.status} #{picked(res.headers)} #{res.body}"
}

fn attempt(req: Request) -> String uses { Http } {
  show(req) escape {
    InvalidUrl(url:) -> "invalid #{url}"
    ConnectionFailed(url:, message:) -> if String.contains(message, "ECONNREFUSED") { "refused" } else { "failed" }
    ResponseNotText(url:) -> "not text"
  }
}

fn main() {
  base = "BASE"
  with Platform {
    c = use Console
    c.print_line(attempt(Request.new(Get, "#{base}/echo")))
    c.print_line(attempt(Request(body: "東京", ..Request.new(Post, "#{base}/echo")) |> Request.add_header("X-One", "1")))
    c.print_line(attempt(Request.new(Head, "#{base}/echo")))
    c.print_line(attempt(Request.new(Get, "#{base}/redirect")))
    c.print_line(attempt(Request.new(Get, "#{base}/missing")))
    c.print_line(attempt(Request.new(Get, "#{base}/binary")))
    c.print_line(attempt(Request.new(Get, "#{base}/loop")))
    c.print_line(attempt(Request.new(Get, "not a url")))
    c.print_line(attempt(Request.new(Get, "ftp://127.0.0.1/")))
    c.print_line(attempt(Request.new(Get, "CLOSED")))
  }
}
"##
    .replace("BASE", &base)
    .replace("CLOSED", &closed);
    let output = run_source("http-system", &main);
    drop(server);
    let echo = "[(\"content-type\", \"text/plain; charset=utf-8\"), (\"set-cookie\", \"a=1\"), (\"set-cookie\", \"b=2\"), (\"x-multi\", \"p, q\")]";
    assert_eq!(
        (output.code, text(&output.stdout), text(&output.stderr)),
        (
            0,
            format!(
                "200 {echo} GET - - \n\
                 200 {echo} POST text/plain;charset=UTF-8 1 東京\n\
                 200 {echo} \n\
                 200 {echo} GET - - \n\
                 404 [] not found 東\n\
                 not text\n\
                 failed\n\
                 invalid not a url\n\
                 invalid ftp://127.0.0.1/\n\
                 refused\n"
            )
            .as_str(),
            ""
        )
    );
}
