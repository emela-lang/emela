//! HTTP services on the JS target (`[lib.http-service]`, `[target.js.module]`, `[cmd.build]`): the
//! `fetch` of the default export of `main.mjs`, called by a harness the way Deno, Bun and Workers
//! call it, and `serve.mjs`, which `emela run` starts.
//!
//! Needs node; the WASM test also needs wasmtime (`EMELA_WASMTIME` or `wasmtime`), and is skipped
//! with a message without it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use emela_driver::{
    CoreJs, CoreWasm, Input, Lower, MemoryFiles, Output, RunOptions, Target, build, run,
    wasmtime_program,
};

/// A per-test output directory. Leftovers from earlier runs are removed.
fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "emela-driver-http-service-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}

/// Builds `main` as `app/src/main.emel` into `dir` and returns the directory.
fn build_service(name: &str, main: &str) -> PathBuf {
    let sources = [("app/Pome.toml", ""), ("app/src/main.emel", main)];
    let fs = MemoryFiles::new(sources.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let dir = out_dir(name);
    let (analysis, entry) = build(&fs, &input, &mut Lower::default(), &mut CoreJs, &dir);
    assert!(
        analysis.diagnostics.is_empty(),
        "{:?}",
        analysis.diagnostics
    );
    assert!(entry.is_some());
    dir
}

/// Runs the node module `harness` next to the output in `dir`: (stdout, stderr).
fn harness(dir: &Path, harness: &str) -> (String, String) {
    std::fs::write(dir.join("harness.mjs"), harness).unwrap();
    let out = Command::new("node")
        .arg("harness.mjs")
        .current_dir(dir)
        .output()
        .expect("cannot start node");
    (text(&out.stdout).to_owned(), text(&out.stderr).to_owned())
}

/// The prelude of a harness: `show(request)` calls `fetch` and prints the status, the headers and
/// the body of the response on one line.
const SHOW: &str = r#"import service from "./main.mjs";
const show = async (request) => {
  const res = await service.fetch(request);
  console.log(res.status, JSON.stringify([...res.headers]), JSON.stringify(await res.text()));
};
"#;

const APP: &str = r##"import Std.Http
import Std.Http.Request.{Request, Get, Post}
import Std.Http.Response.{Response}
import Std.Console.{Console, SystemConsole}
import Std.Clock.{Clock, SystemClock}

error Teapot(reason: String) derive Show

effect Note {
  fn note(text: String) -> ()
}

handler Noted: Note {
  init {
    (use Console).print_line("init")
    Noted
  }
  fn note(text) { (use Console).print_line(text) }
  release { (use Console).print_line("release") }
}

@entry(Http.serve)
pub fn app(request: Request) -> Response fails Teapot {
  with SystemConsole, SystemClock, Noted {
    (use Note).note("#{request.method} #{request.url}")
    (request.method, Request.segments(request)) match {
      (Get, ["echo", ..rest]) -> Response.new(
        200,
        "#{rest} #{Request.query(request)} #{request.headers} #{request.body}",
      )
      (Post, ["echo"]) -> Response.new(200, request.body)
      (_, ["slow", ms]) -> {
        (use Clock).sleep(Int.parse(ms) escape { ParseError(input: _) -> 0 })
        (use Note).note("woke #{ms}")
        Response.new(200, ms)
      }
      (_, ["typed"]) -> Response.new(201, "{}") |> Response.add_header("Content-Type", "application/json")
      (_, ["empty"]) -> Response.new(204, "")
      (_, ["headers"]) -> Response.new(200, "")
        |> Response.add_header("x-b", "1")
        |> Response.add_header("x-a", "2")
        |> Response.add_header("x-a", "3")
        |> Response.add_header("set-cookie", "a=1")
        |> Response.add_header("set-cookie", "b=2")
      (_, ["fail"]) -> fail Teapot(reason: "short and stout")
      (_, ["defect"]) -> panic("boom")
      (_, ["status"]) -> Response.new(99, "")
      (_, ["no-content"]) -> Response.new(204, "x")
      (_, ["name"]) -> Response.new(200, "") |> Response.add_header("bad name", "x")
      (_, ["value"]) -> Response.new(200, "") |> Response.add_header("x-v", "a\nb")
      (_, ["latin"]) -> Response.new(200, "") |> Response.add_header("x-v", "é")
      _ -> Response.new(404, "")
    }
  }
}
"##;

/// The conversions of a request and a response (`[lib.http-service]`).
#[test]
fn fetch_converts_requests_and_responses() {
    let dir = build_service("convert", APP);
    let (stdout, stderr) = harness(
        &dir,
        &format!(
            r#"{SHOW}
await show(new Request("http://example.com/echo/a%20b/%E6%9D%B1?q=%E6%9D%B1+x&flag", {{
  headers: [["X-B", "2"], ["x-a", "1"], ["X-A", "0"], ["Set-Cookie", "s=1"], ["set-cookie", "t=2"]],
}}));
await show(new Request("http://example.com/echo", {{ method: "POST", body: "﻿東京" }}));
await show(new Request("http://example.com/typed"));
await show(new Request("http://example.com/empty"));
await show(new Request("http://example.com/headers"));
await show(new Request("http://example.com/echo", {{ method: "PROPFIND" }}));
await show(new Request("http://example.com/echo", {{ method: "POST", body: new Uint8Array([0x41, 0xff]) }}));
await show(new Request("http://example.com/echo/%zz"));
await show(new Request("http://example.com/echo/%C0%80"));
await show(new Request("http://example.com/echo?a=%E6"));
"#
        ),
    );
    let _ = std::fs::remove_dir_all(&dir);
    let text_plain = r#"["content-type","text/plain;charset=UTF-8"]"#;
    assert_eq!(
        stdout,
        format!(
            "init\nGet http://example.com/echo/a%20b/%E6%9D%B1?q=%E6%9D%B1+x&flag\nrelease\n\
             200 [{text_plain}] \"[\\\"a b\\\", \\\"東\\\"] [(\\\"q\\\", \\\"東 x\\\"), (\\\"flag\\\", \\\"\\\")] [(\\\"set-cookie\\\", \\\"s=1\\\"), (\\\"set-cookie\\\", \\\"t=2\\\"), (\\\"x-a\\\", \\\"1, 0\\\"), (\\\"x-b\\\", \\\"2\\\")] \"\n\
             init\nPost http://example.com/echo\nrelease\n\
             200 [{text_plain}] \"東京\"\n\
             init\nGet http://example.com/typed\nrelease\n\
             201 [[\"content-type\",\"application/json\"]] \"{{}}\"\n\
             init\nGet http://example.com/empty\nrelease\n\
             204 [] \"\"\n\
             init\nGet http://example.com/headers\nrelease\n\
             200 [[\"set-cookie\",\"a=1\"],[\"set-cookie\",\"b=2\"],[\"x-a\",\"2, 3\"],[\"x-b\",\"1\"]] \"\"\n\
             405 [[\"allow\",\"GET, HEAD, POST, PUT, PATCH, DELETE, OPTIONS\"],{text_plain}] \"Method Not Allowed\"\n\
             400 [{text_plain}] \"Bad Request\"\n\
             400 [{text_plain}] \"Bad Request\"\n\
             400 [{text_plain}] \"Bad Request\"\n\
             400 [{text_plain}] \"Bad Request\"\n"
        )
    );
    assert_eq!(stderr, "");
}

/// An error, a defect, or a response the host cannot send: one line on standard error and 500,
/// without the cause in the response. The `with` inside the app releases first.
#[test]
fn fetch_answers_failures_with_500() {
    let dir = build_service("failures", APP);
    let (stdout, stderr) = harness(
        &dir,
        &format!(
            r#"{SHOW}
for (const path of ["fail", "defect", "status", "no-content", "name", "value", "latin"]) {{
  await show(new Request(`http://example.com/${{path}}`));
}}
"#
        ),
    );
    let _ = std::fs::remove_dir_all(&dir);
    let internal =
        "500 [[\"content-type\",\"text/plain;charset=UTF-8\"]] \"Internal Server Error\"\n";
    let launches: String = [
        "fail",
        "defect",
        "status",
        "no-content",
        "name",
        "value",
        "latin",
    ]
    .iter()
    .map(|path| format!("init\nGet http://example.com/{path}\nrelease\n{internal}"))
    .collect();
    assert_eq!(stdout, launches);
    assert_eq!(
        stderr,
        "error: Teapot(reason: \"short and stout\")\n\
         defect: boom\n\
         invalid response: status 99\n\
         invalid response: status 204 cannot have a body\n\
         invalid response: header name `bad name`\n\
         invalid response: header value of `x-v`\n\
         invalid response: header value of `x-v`\n"
    );
}

/// The host may call again before an earlier call returns; each call is its own launch, and they
/// switch at suspension points (`[lib.http-service]`).
#[test]
fn fetch_runs_launches_concurrently() {
    let dir = build_service("concurrent", APP);
    let (stdout, stderr) = harness(
        &dir,
        &format!(
            r#"{SHOW}
await Promise.all([
  show(new Request("http://example.com/slow/40")),
  show(new Request("http://example.com/slow/1")),
]);
"#
        ),
    );
    let _ = std::fs::remove_dir_all(&dir);
    let ok =
        |body: &str| format!("200 [[\"content-type\",\"text/plain;charset=UTF-8\"]] \"{body}\"\n");
    assert_eq!(
        stdout,
        format!(
            "init\nGet http://example.com/slow/40\ninit\nGet http://example.com/slow/1\n\
             woke 1\nrelease\n{}woke 40\nrelease\n{}",
            ok("1"),
            ok("40")
        )
    );
    assert_eq!(stderr, "");
}

/// The server `emela run` starts (`[target.js.module]`, `[cmd.build]`).
struct Server {
    child: Child,
    port: u16,
}

impl Server {
    /// Starts `serve.mjs` in `dir` with `args` and `env`, and waits for its `listening on` line.
    fn start(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Server {
        let mut command = Command::new("node");
        command
            .arg("serve.mjs")
            .args(args)
            .current_dir(dir)
            .env_remove("HOST")
            .env_remove("PORT")
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        for (name, value) in env {
            command.env(name, value);
        }
        let mut child = command.spawn().expect("cannot start node");
        let mut line = String::new();
        BufReader::new(child.stderr.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let port = line
            .trim_end()
            .strip_prefix("listening on http://127.0.0.1:")
            .unwrap_or_else(|| panic!("unexpected first line {line:?}"))
            .parse()
            .unwrap();
        Server { child, port }
    }

    /// Sends `request` (an HTTP/1.1 request without `host` and `connection`) and returns the
    /// status line, the headers other than date, connection, keep-alive and content-length, and
    /// the body.
    fn send(&self, request: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let (head, body) = request.split_once("\r\n\r\n").unwrap_or((request, ""));
        write!(
            stream,
            "{head}\r\nhost: 127.0.0.1\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        let kept: Vec<&str> = head
            .lines()
            .filter(|line| {
                let name = line.split(':').next().unwrap().to_ascii_lowercase();
                !matches!(
                    name.as_str(),
                    "date" | "connection" | "keep-alive" | "content-length" | "transfer-encoding"
                )
            })
            .collect();
        format!("{}\n{body}", kept.join("\n"))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn serve_mjs_passes_requests_to_fetch() {
    let dir = build_service("serve-mjs", APP);
    assert!(dir.join("serve.mjs").exists());
    let server = Server::start(&dir, &["--addr", "127.0.0.1:0"], &[]);
    let echo = server.send("GET /echo/x?y=1 HTTP/1.1\r\nX-One: 1");
    let post = server.send("POST /echo HTTP/1.1\r\n\r\nbody");
    let headers = server.send("GET /headers HTTP/1.1");
    let trace = server.send("TRACE / HTTP/1.1");
    let defect = server.send("GET /defect HTTP/1.1");
    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        echo,
        "HTTP/1.1 200 OK\ncontent-type: text/plain;charset=UTF-8\n[\"x\"] [(\"y\", \"1\")] \
         [(\"content-length\", \"0\"), (\"x-one\", \"1\")] "
    );
    assert_eq!(
        post,
        "HTTP/1.1 200 OK\ncontent-type: text/plain;charset=UTF-8\nbody"
    );
    assert_eq!(
        headers,
        "HTTP/1.1 200 OK\nx-a: 2, 3\nx-b: 1\nset-cookie: a=1\nset-cookie: b=2\n"
    );
    assert_eq!(
        trace,
        "HTTP/1.1 405 Method Not Allowed\nallow: GET, HEAD, POST, PUT, PATCH, DELETE, OPTIONS\n\
         content-type: text/plain;charset=UTF-8\nMethod Not Allowed"
    );
    assert_eq!(
        defect,
        "HTTP/1.1 500 Internal Server Error\ncontent-type: text/plain;charset=UTF-8\nInternal Server Error"
    );
}

/// The address: `--addr`, otherwise HOST and PORT (either alone), otherwise 127.0.0.1:8080. A
/// malformed one stops the server with a message.
#[test]
fn serve_mjs_address() {
    let dir = build_service("serve-address", APP);
    let server = Server::start(&dir, &[], &[("PORT", "0")]);
    assert_ne!(server.port, 8080);
    drop(server);
    let server = Server::start(&dir, &[], &[("HOST", "127.0.0.1"), ("PORT", "0")]);
    drop(server);
    let server = Server::start(&dir, &["--addr", "127.0.0.1:0"], &[("PORT", "1")]);
    drop(server);
    let bad = |args: &[&str], env: &[(&str, &str)]| {
        let mut command = Command::new("node");
        command.arg("serve.mjs").args(args).current_dir(&dir);
        command.env_remove("HOST").env_remove("PORT");
        for (name, value) in env {
            command.env(name, value);
        }
        let out = command.output().unwrap();
        (out.status.code(), text(&out.stderr).to_owned())
    };
    let addr = bad(&["--addr", "8080"], &[]);
    let port = bad(&[], &[("PORT", "http")]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        addr,
        (
            Some(2),
            "error: --addr takes <host>:<port>, got `8080`\n".to_owned()
        )
    );
    assert_eq!(
        port,
        (
            Some(2),
            "error: PORT must be a port number, got `http`\n".to_owned()
        )
    );
}

/// `emela run` of an HTTP service needs no `main` (no E0230) and runs `node serve.mjs` with the
/// arguments after `--` (`[cmd.build]`). `echo` stands in for node, so the command shows.
#[test]
fn run_starts_serve_mjs() {
    let sources = [("app/Pome.toml", ""), ("app/src/main.emel", APP)];
    let fs = MemoryFiles::new(sources.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = RunOptions {
        out_dir: out_dir("run"),
        node: PathBuf::from("echo"),
        args: vec!["--addr".into(), "127.0.0.1:0".into()],
        output: Output::Capture,
    };
    let (analysis, output) = run(
        &fs,
        &input,
        &mut Lower::default(),
        &mut CoreJs,
        &options,
        |_, _| {},
    );
    let serve = options.out_dir.join("serve.mjs");
    let exists = serve.exists();
    let _ = std::fs::remove_dir_all(&options.out_dir);
    assert!(
        analysis.diagnostics.is_empty(),
        "{:?}",
        analysis.diagnostics
    );
    assert!(exists);
    assert_eq!(
        text(&output.unwrap().stdout),
        format!("{} --addr 127.0.0.1:0\n", serve.display())
    );
}

/// Without `@entry(Http.serve)`, no `serve.mjs` is written.
#[test]
fn no_serve_mjs_without_a_service() {
    let dir = build_service("no-service", "fn main() {}\n");
    let exists = dir.join("serve.mjs").exists();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!exists);
}

/// The code of `[lib.http-service]` and of `[api.std.http]`, with the imports and the fn they
/// leave out, type-checks (these examples use std, which the type checker's own spec examples do
/// not load).
#[test]
fn spec_examples_check() {
    let service = r##"import Std.Http
import Std.Http.Request.{Request}
import Std.Http.Response.{Response}
import Std.Platform.{Platform}

fn route(request: Request) -> Response { Response.new(200, request.url) } # 補

@entry(Http.serve)
pub fn app(request: Request) -> Response {
  with Platform { route(request) }
}
"##;
    let client = r##"import Std.Http.{Http, TestServer, TestHttp}
import Std.Http.Response
import Std.State.{State}

@test
fn fetches() {
  server = TestServer(respond: fn(req, n) { Some(Response.new(200, "ok")) }, sent: [])
  (res, after) = State.run(server, fn() { with TestHttp { Http.get("https://example.com") } })
  assert res.body == "ok"
  assert List.length(after.sent) == 1
}

fn main() {}
"##;
    for (name, main) in [("spec-service", service), ("spec-client", client)] {
        let dir = build_service(name, main);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// A server of the service, for the test that compares the targets: `serve.mjs` on node, or
/// `wasmtime serve`. Standard output and standard error are collected in a file.
struct Served {
    child: Child,
    base: String,
    log: PathBuf,
}

impl Served {
    /// Starts `command` with its output in `log`, and waits for the line that names the address
    /// (`listening on <base>` or `Serving HTTP on <base>/`).
    fn start(mut command: Command, log: PathBuf) -> Served {
        let file = std::fs::File::create(&log).unwrap();
        command
            .stdout(file.try_clone().unwrap())
            .stderr(file)
            .env_remove("HOST")
            .env_remove("PORT");
        // Made before waiting, so that `Drop` stops the server if it does not start.
        let mut served = Served {
            child: command.spawn().expect("cannot start the server"),
            base: String::new(),
            log,
        };
        let started = std::time::Instant::now();
        loop {
            let text = std::fs::read_to_string(&served.log).unwrap_or_default();
            let found = text.lines().find_map(|line| {
                line.strip_prefix("listening on ")
                    .or_else(|| line.strip_prefix("Serving HTTP on "))
            });
            if let Some(base) = found {
                served.base = base.trim_end_matches('/').to_owned();
                return served;
            }
            assert!(
                started.elapsed().as_secs() < 30,
                "the server did not start:\n{text}"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// The lines the program wrote, without the address and without the lines of the server
    /// itself. `wasmtime serve` puts `stdout [n] :: ` or `stderr [n] :: ` before each line.
    fn program_lines(&self) -> String {
        let text = std::fs::read_to_string(&self.log).unwrap();
        let port = self.base.rsplit(':').next().unwrap();
        text.lines()
            .filter(|line| {
                !line.starts_with("listening on ") && !line.starts_with("Serving HTTP on ")
            })
            .map(|line| {
                let line = match line.split_once(" :: ") {
                    Some((prefix, rest)) if prefix.starts_with("std") => rest,
                    _ => line,
                };
                format!("{}\n", line.replace(port, "PORT"))
            })
            .collect()
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Sends the requests of `REQUESTS` to `base` one after another with the Fetch API of node, and
/// prints each response: the status, the headers other than date and the framing ones, sorted,
/// and the body. The port is written as `PORT`.
fn fetch_all(dir: &Path, base: &str) -> String {
    let script = format!(
        r#"const base = {base:?};
const port = new URL(base).port;
const requests = [
  ["GET", "/echo/a%20b/%E6%9D%B1?q=%E6%9D%B1+x&flag", {{ "X-B": "2", "x-a": "1", "X-L": "\u00e9" }}],
  ["POST", "/echo", {{}}, "\uFEFF東京"],
  ["POST", "/echo", {{}}, new Uint8Array([0x41, 0xff])],
  ["GET", "/typed"],
  ["GET", "/empty"],
  ["GET", "/headers"],
  ["PROPFIND", "/echo"],
  ["GET", "/echo/%zz"],
  ["GET", "/echo?a=%E6"],
  ["GET", "/fail"],
  ["GET", "/defect"],
  ["GET", "/status"],
  ["GET", "/no-content"],
  ["GET", "/name"],
  ["GET", "/value"],
  ["GET", "/slow/5"],
];
const skip = new Set(["date", "connection", "keep-alive", "content-length", "transfer-encoding"]);
for (const [method, path, headers = {{}}, body] of requests) {{
  const res = await fetch(base + path, {{ method, headers, body }});
  const kept = [...res.headers].filter(([name]) => !skip.has(name)).sort();
  const text = (await res.text()).replaceAll(port, "PORT");
  console.log(method, path, res.status, JSON.stringify(kept), JSON.stringify(text));
}}
"#
    );
    std::fs::write(dir.join("fetch_all.mjs"), script).unwrap();
    let out = Command::new("node")
        .arg("fetch_all.mjs")
        .current_dir(dir)
        .output()
        .expect("cannot start node");
    assert_eq!(text(&out.stderr), "");
    text(&out.stdout).to_owned()
}

/// Whether wasmtime starts; prints why the test is skipped otherwise.
fn have_wasmtime() -> bool {
    let wasmtime = wasmtime_program();
    let ok = Command::new(&wasmtime)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("skipping: cannot start `{}`", wasmtime.display());
    }
    ok
}

/// The same requests give the same responses and the same output on JS (`serve.mjs`) and on WASM
/// (`wasmtime serve`, `[target.wasm.output]`, `[target.wasm.run]`). Only the order of the response
/// headers may differ (`[lib.http-service]`), so they are compared sorted, and the authority of
/// `url`, which `wasmtime serve` does not give.
#[test]
fn wasm_serves_the_same_as_js() {
    if !have_wasmtime() {
        return;
    }
    let js = build_service("same-js", APP);
    let sources = [("app/Pome.toml", ""), ("app/src/main.emel", APP)];
    let fs = MemoryFiles::new(sources.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let wasm = out_dir("same-wasm");
    let (analysis, entry) = build(
        &fs,
        &input,
        &mut Lower::default().with_target(Target::Wasm),
        &mut CoreWasm,
        &wasm,
    );
    assert!(
        analysis.diagnostics.is_empty(),
        "{:?}",
        analysis.diagnostics
    );
    let component = entry.unwrap();

    let mut node = Command::new("node");
    node.arg("serve.mjs")
        .arg("--addr")
        .arg("127.0.0.1:0")
        .current_dir(&js);
    let served = Served::start(node, js.join("served.log"));
    let js_responses = fetch_all(&js, &served.base);
    std::thread::sleep(std::time::Duration::from_millis(100));
    let js_lines = served.program_lines();
    drop(served);

    let mut wasmtime = Command::new(wasmtime_program());
    wasmtime
        .args(["serve", "-S", "p3=y,cli=y", "--addr", "127.0.0.1:0"])
        .arg(&component);
    let served = Served::start(wasmtime, wasm.join("served.log"));
    let wasm_responses = fetch_all(&wasm, &served.base);
    std::thread::sleep(std::time::Duration::from_millis(100));
    let wasm_lines = served.program_lines();
    drop(served);
    let _ = std::fs::remove_dir_all(&js);
    let _ = std::fs::remove_dir_all(&wasm);

    assert_eq!(wasm_responses, js_responses);
    // `wasmtime serve` gives no authority, so `url` has an empty one there (`[lib.http-service]`).
    assert_eq!(
        wasm_lines,
        js_lines.replace("http://127.0.0.1:PORT/", "http:///")
    );
    insta::assert_snapshot!(format!("{js_responses}---\n{js_lines}"));
}

const STALLS: &str = r##"import Std.Http
import Std.Http.Request.{Request}
import Std.Http.Response.{Response}

effect Wait {
  suspend fn wait() -> ()
}

## Never resumes: the launch stalls and ends interrupted.
handler Forgets: Wait {
  suspend fn wait(resume) { () }
}

@entry(Http.serve)
pub fn app(request: Request) -> Response {
  if Request.path(request) == "/stall" {
    with Forgets { (use Wait).wait() }
  }
  Response.new(200, "ok")
}
"##;

/// A launch that ends interrupted makes no response: the host gets an error. On JS the Promise
/// of `fetch` rejects, and `serve.mjs` writes `interrupted` and answers with 500; on WASM `handle`
/// returns `error-code.internal-error`, which `wasmtime serve` answers with 500
/// (`[lib.http-service]`). The next request is served as usual.
#[test]
fn an_interrupted_launch_is_a_host_error() {
    let js = build_service("stall-js", STALLS);
    let (stdout, stderr) = harness(
        &js,
        &format!(
            r#"{SHOW}
await service.fetch(new Request("http://example.com/stall")).catch((e) => console.log("rejected", e.name));
await show(new Request("http://example.com/ok"));
"#
        ),
    );
    let mut node = Command::new("node");
    node.arg("serve.mjs")
        .arg("--addr")
        .arg("127.0.0.1:0")
        .current_dir(&js);
    let served = Served::start(node, js.join("served.log"));
    let js_statuses = statuses(&served.base);
    std::thread::sleep(std::time::Duration::from_millis(100));
    let lines = served.program_lines();
    drop(served);
    let _ = std::fs::remove_dir_all(&js);
    assert_eq!(
        stdout,
        "rejected Interrupted\n200 [[\"content-type\",\"text/plain;charset=UTF-8\"]] \"ok\"\n"
    );
    assert_eq!(stderr, "");
    assert_eq!(js_statuses, "500 200");
    assert_eq!(lines, "interrupted\n");

    if !have_wasmtime() {
        return;
    }
    let sources = [("app/Pome.toml", ""), ("app/src/main.emel", STALLS)];
    let fs = MemoryFiles::new(sources.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let wasm = out_dir("stall-wasm");
    let (analysis, entry) = build(
        &fs,
        &input,
        &mut Lower::default().with_target(Target::Wasm),
        &mut CoreWasm,
        &wasm,
    );
    assert!(
        analysis.diagnostics.is_empty(),
        "{:?}",
        analysis.diagnostics
    );
    let mut wasmtime = Command::new(wasmtime_program());
    wasmtime
        .args(["serve", "-S", "p3=y,cli=y", "--addr", "127.0.0.1:0"])
        .arg(entry.unwrap());
    let served = Served::start(wasmtime, wasm.join("served.log"));
    let wasm_statuses = statuses(&served.base);
    drop(served);
    let _ = std::fs::remove_dir_all(&wasm);
    assert_eq!(wasm_statuses, "500 200");
}

/// The statuses of `/stall` and then `/ok` at `base`.
fn statuses(base: &str) -> String {
    let script = format!(
        r#"const codes = [];
for (const path of ["/stall", "/ok"]) codes.push((await fetch({base:?} + path)).status);
console.log(codes.join(" "));
"#
    );
    let out = Command::new("node")
        .arg("--input-type=module")
        .arg("-e")
        .arg(script)
        .output()
        .expect("cannot start node");
    text(&out.stdout).trim_end().to_owned()
}
