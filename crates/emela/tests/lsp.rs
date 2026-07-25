//! Integration tests for `emela lsp` (spec 0033): drive the server binary
//! over stdio with framed JSON-RPC, the way an editor would.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-lsp-test-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    // The server canonicalizes module paths, so tests must too (macOS's
    // temp dir is behind a symlink).
    dir.canonicalize().unwrap()
}

fn uri_of(path: &Path) -> String {
    format!("file://{}", path.display())
}

struct Lsp {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: i64,
}

impl Lsp {
    fn start() -> Lsp {
        let mut child = Command::new(env!("CARGO_BIN_EXE_emela"))
            .arg("lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let reader = BufReader::new(child.stdout.take().unwrap());
        let mut lsp = Lsp {
            child,
            stdin,
            reader,
            next_id: 0,
        };
        let result = lsp.request("initialize", json!({"capabilities": {}}));
        assert!(
            result["capabilities"]["completionProvider"].is_object(),
            "{result}"
        );
        assert_eq!(result["capabilities"]["hoverProvider"], json!(true));
        assert!(
            result["capabilities"]["codeActionProvider"].is_object(),
            "{result}"
        );
        lsp.notify("initialized", json!({}));
        lsp
    }

    fn send(&mut self, payload: &Value) {
        let body = serde_json::to_vec(payload).unwrap();
        write!(self.stdin, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
        self.stdin.write_all(&body).unwrap();
        self.stdin.flush().unwrap();
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn read_message(&mut self) -> Value {
        let mut length = None;
        loop {
            let mut line = String::new();
            assert!(self.reader.read_line(&mut line).unwrap() > 0, "server EOF");
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length:") {
                length = Some(value.trim().parse::<usize>().unwrap());
            }
        }
        let mut body = vec![0u8; length.expect("Content-Length")];
        self.reader.read_exact(&mut body).unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    /// Sends a request and reads until its response, skipping notifications.
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let message = self.read_message();
            if message["id"] == json!(id) {
                assert!(
                    message.get("error").is_none(),
                    "request `{method}` failed: {message}"
                );
                return message["result"].clone();
            }
        }
    }

    fn open(&mut self, uri: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri, "languageId": "emela", "version": 1, "text": text}}),
        );
    }

    fn change(&mut self, uri: &str, version: i64, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"text": text}],
            }),
        );
    }

    fn close(&mut self, uri: &str) {
        self.notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": uri}}),
        );
    }

    /// Reads notifications until diagnostics for `uri` arrive.
    fn wait_diagnostics(&mut self, uri: &str) -> Vec<Value> {
        loop {
            let message = self.read_message();
            if message["method"] == json!("textDocument/publishDiagnostics")
                && message["params"]["uri"] == json!(uri)
            {
                return message["params"]["diagnostics"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
            }
        }
    }

    fn hover(&mut self, uri: &str, line: u32, character: u32) -> Value {
        self.request(
            "textDocument/hover",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character},
            }),
        )
    }

    /// Requests code actions for a collapsed range at a position.
    fn code_actions(&mut self, uri: &str, line: u32, character: u32) -> Value {
        self.request(
            "textDocument/codeAction",
            json!({
                "textDocument": {"uri": uri},
                "range": {
                    "start": {"line": line, "character": character},
                    "end": {"line": line, "character": character},
                },
                "context": {"diagnostics": []},
            }),
        )
    }

    /// The hover's code-block content at a position; panics on a null hover.
    fn hover_value(&mut self, uri: &str, line: u32, character: u32) -> String {
        let result = self.hover(uri, line, character);
        result["contents"]["value"]
            .as_str()
            .unwrap_or_else(|| panic!("expected hover at {line}:{character}, got {result}"))
            .to_string()
    }

    fn completion_labels(&mut self, uri: &str, line: u32, character: u32) -> Vec<String> {
        let result = self.request(
            "textDocument/completion",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character},
            }),
        );
        result
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["label"].as_str().unwrap().to_string())
            .collect()
    }

    fn shutdown_and_exit(mut self) {
        self.request("shutdown", Value::Null);
        self.notify("exit", json!({}));
        let status = self.child.wait().unwrap();
        assert_eq!(status.code(), Some(0), "exit after shutdown should be 0");
    }

    /// Replaces the document with a fresh one-function body containing `line`
    /// and completes at its end — a helper for cursor-after-text cases.
    fn completion_labels_at_extra(&mut self, uri: &str, line: &str) -> Vec<String> {
        let text = format!("fn main() -> Int uses {{}} {{\n{line}\n}}\n");
        self.change(uri, 99, &text);
        self.wait_diagnostics(uri);
        self.completion_labels(uri, 1, line.len() as u32)
    }
}

impl Drop for Lsp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn messages_of(diagnostics: &[Value]) -> Vec<String> {
    diagnostics
        .iter()
        .map(|d| d["message"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn lifecycle_initialize_shutdown_exit() {
    let lsp = Lsp::start();
    lsp.shutdown_and_exit();
}

#[test]
fn publishes_multiple_diagnostics_and_clears_on_fix() {
    let dir = temp_dir();
    let path = dir.join("main.emel");
    let uri = uri_of(&path);
    fs::write(&path, "").unwrap();
    let mut lsp = Lsp::start();

    lsp.open(
        &uri,
        r#"
fn f() -> Int uses {} {
  "text"
}

fn g() -> Int uses {} {
  unknown_name
}

fn main() -> Int uses {} {
  f() + g()
}
"#,
    );
    let diagnostics = lsp.wait_diagnostics(&uri);
    let messages = messages_of(&diagnostics);
    assert_eq!(diagnostics.len(), 2, "{messages:?}");
    assert!(
        messages.iter().any(|m| m.contains("Type mismatch")),
        "{messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("Unknown name")),
        "{messages:?}"
    );
    // The `"text"` literal sits on line 2 (0-based), columns 2..8.
    let range = &diagnostics
        .iter()
        .find(|d| d["message"].as_str().unwrap().contains("Type mismatch"))
        .unwrap()["range"];
    assert_eq!(
        range["start"],
        json!({"line": 2, "character": 2}),
        "{range}"
    );
    assert_eq!(range["end"], json!({"line": 2, "character": 8}), "{range}");

    lsp.change(
        &uri,
        2,
        r#"
fn main() -> Int uses {} {
  42
}
"#,
    );
    assert!(lsp.wait_diagnostics(&uri).is_empty());
    let _ = fs::remove_dir_all(&dir);
    lsp.shutdown_and_exit();
}

// One representative source per compiler error category (spec 0033): every
// stage's errors surface as diagnostics with their title in the message.
#[test]
fn covers_every_error_category() {
    let cases: &[(&str, &str)] = &[
        // `@` is an attribute prefix now (spec 0039), so `#` is the
        // representative unknown character.
        (
            "Unexpected character",
            "fn main() -> Int uses {} {\n  1 # 2\n}\n",
        ),
        (
            "Expected an expression",
            "fn main() -> Int uses {} {\n  let x =\n}\n",
        ),
        ("Type mismatch", "fn f() -> Int uses {} {\n  \"x\"\n}\n"),
        (
            "Non-exhaustive match",
            "enum Color {\n  Red\n  Green\n}\n\nfn f(c: Color) -> Int uses {} {\n  match c {\n    Red -> 1\n  }\n}\n",
        ),
        (
            "Incomplete impl",
            "trait Greet {\n  fn greet(x: Self) -> String\n}\n\nenum Foo {\n  A\n}\n\nimpl Greet for Foo {\n}\n",
        ),
        (
            "Unhandled effects",
            "effect Log {\n  pub fn info() -> Unit {\n    ()\n  }\n}\n\nfn a() -> Unit uses { Log } {\n  ()\n}\n\nfn b() -> Unit uses {} {\n  a()\n}\n",
        ),
        (
            "Unhandled throwing call",
            "enum E {\n  X\n}\n\nfn t() -> Int throws E uses {} {\n  throw E::X\n}\n\nfn u() -> Int uses {} {\n  t()\n}\n",
        ),
        (
            "Invalid entrypoint",
            "fn main(x: Int) -> Int uses {} {\n  x\n}\n",
        ),
        (
            "Unknown variant",
            "enum Color {\n  Red\n}\n\nfn f() -> Color uses {} {\n  Color::Blue\n}\n",
        ),
    ];
    let dir = temp_dir();
    let mut lsp = Lsp::start();
    for (index, (title, source)) in cases.iter().enumerate() {
        let path = dir.join(format!("case{index}.emel"));
        fs::write(&path, source).unwrap();
        let uri = uri_of(&path);
        lsp.open(&uri, source);
        let diagnostics = lsp.wait_diagnostics(&uri);
        let messages = messages_of(&diagnostics);
        assert!(
            messages.iter().any(|m| m.contains(title)),
            "expected `{title}` in {messages:?}"
        );
        lsp.close(&uri);
        lsp.wait_diagnostics(&uri);
    }
    let _ = fs::remove_dir_all(&dir);
    lsp.shutdown_and_exit();
}

// An error inside an imported module is published at that module's URI, and
// the entry file gets a summary pointing at it.
#[test]
fn routes_imported_module_errors() {
    let dir = temp_dir();
    fs::write(
        dir.join("geometry.emel"),
        "module geometry\n\npub fn square(n: Int) -> Int {\n  \"oops\"\n}\n",
    )
    .unwrap();
    let main_path = dir.join("main.emel");
    let main_source = "import geometry\n\nfn main() -> Int uses {} {\n  geometry.square(5)\n}\n";
    fs::write(&main_path, main_source).unwrap();
    let main_uri = uri_of(&main_path);
    let geometry_uri = uri_of(&dir.join("geometry.emel"));

    let mut lsp = Lsp::start();
    lsp.open(&main_uri, main_source);
    let main_diagnostics = lsp.wait_diagnostics(&main_uri);
    let geometry_diagnostics = lsp.wait_diagnostics(&geometry_uri);
    let main_messages = messages_of(&main_diagnostics);
    let geometry_messages = messages_of(&geometry_diagnostics);
    assert!(
        geometry_messages
            .iter()
            .any(|m| m.contains("Type mismatch")),
        "{geometry_messages:?}"
    );
    assert!(
        main_messages.iter().any(|m| m.contains("imported module")),
        "{main_messages:?}"
    );
    let _ = fs::remove_dir_all(&dir);
    lsp.shutdown_and_exit();
}

fn open_and_settle(lsp: &mut Lsp, uri: &str, text: &str) {
    lsp.open(uri, text);
    lsp.wait_diagnostics(uri);
}

// Completion context 6 (default): keywords and in-scope names.
#[test]
fn completes_keywords_and_scope() {
    let dir = temp_dir();
    let path = dir.join("main.emel");
    let uri = uri_of(&path);
    fs::write(&path, "").unwrap();
    let source = "fn add(x: Int, y: Int) -> Int uses {} {\n  x + y\n}\n\nfn main() -> Int uses {} {\n  le\n}\n";
    let mut lsp = Lsp::start();
    open_and_settle(&mut lsp, &uri, source);
    // Cursor after `le` on line 5.
    let labels = lsp.completion_labels(&uri, 5, 4);
    assert!(labels.iter().any(|l| l == "let"), "{labels:?}");
    assert!(labels.iter().any(|l| l == "match"), "{labels:?}");
    assert!(labels.iter().any(|l| l == "add"), "{labels:?}");
    assert!(labels.iter().any(|l| l == "Int"), "{labels:?}");
    let _ = fs::remove_dir_all(&dir);
    lsp.shutdown_and_exit();
}

// Completion context 4: variants of the match scrutinee's enum.
#[test]
fn completes_match_arms_from_scrutinee_type() {
    let dir = temp_dir();
    let path = dir.join("main.emel");
    let uri = uri_of(&path);
    fs::write(&path, "").unwrap();
    let source = "enum Color {\n  Red\n  Green\n  Blue\n}\n\nfn f(c: Color) -> Int uses {} {\n  match c {\n    \n  }\n}\n";
    let mut lsp = Lsp::start();
    open_and_settle(&mut lsp, &uri, source);
    // Cursor on the empty arm line (line 8).
    let labels = lsp.completion_labels(&uri, 8, 4);
    for variant in ["Color::Red", "Color::Green", "Color::Blue"] {
        assert!(labels.iter().any(|l| l == variant), "{labels:?}");
    }
    assert!(labels.iter().any(|l| l == "_"), "{labels:?}");
    let _ = fs::remove_dir_all(&dir);
    lsp.shutdown_and_exit();
}

// Completion context 5: catch arms prefer enums from `throws` clauses.
#[test]
fn completes_catch_arms_with_error_enums() {
    let dir = temp_dir();
    let path = dir.join("main.emel");
    let uri = uri_of(&path);
    fs::write(&path, "").unwrap();
    let source = "enum ParseError {\n  Empty\n  BadDigit\n}\n\nfn parse(s: String) -> Int throws ParseError uses {} {\n  throw ParseError::Empty\n}\n\nfn main() -> Int uses {} {\n  try {\n    parse(\"x\")\n  } catch {\n    \n  }\n}\n";
    let mut lsp = Lsp::start();
    open_and_settle(&mut lsp, &uri, source);
    // Cursor on the empty catch-arm line (line 13).
    let labels = lsp.completion_labels(&uri, 13, 4);
    assert!(
        labels.iter().any(|l| l == "ParseError::Empty"),
        "{labels:?}"
    );
    assert!(
        labels.iter().any(|l| l == "ParseError::BadDigit"),
        "{labels:?}"
    );
    let _ = fs::remove_dir_all(&dir);
    lsp.shutdown_and_exit();
}

// Completion context 3: effect names inside a `uses { … }` row.
#[test]
fn completes_effects_in_uses_row() {
    let dir = temp_dir();
    let path = dir.join("main.emel");
    let uri = uri_of(&path);
    fs::write(&path, "").unwrap();
    let source = "fn log() -> Unit uses { Io } {\n  ()\n}\n\nfn tick() -> Unit uses { Clock } {\n  ()\n}\n\nfn main() -> Unit uses {  } {\n  ()\n}\n";
    let mut lsp = Lsp::start();
    open_and_settle(&mut lsp, &uri, source);
    // Cursor inside `uses {  }` of main (line 8, between the braces).
    let labels = lsp.completion_labels(&uri, 8, 24);
    assert!(labels.iter().any(|l| l == "Io"), "{labels:?}");
    assert!(labels.iter().any(|l| l == "Clock"), "{labels:?}");
    assert!(!labels.iter().any(|l| l == "let"), "{labels:?}");
    let _ = fs::remove_dir_all(&dir);
    lsp.shutdown_and_exit();
}

// Completion context 2: `Enum::` lists its variants. The former
// `Char::`/`String::` conversions are now bare intrinsics (spec 0021), so `::`
// offers no completions on a non-enum type name.
#[test]
fn completes_type_paths() {
    let dir = temp_dir();
    let path = dir.join("main.emel");
    let uri = uri_of(&path);
    fs::write(&path, "").unwrap();
    let source = "enum ParseError {\n  Empty\n  BadDigit\n}\n\nfn main() -> Int uses {} {\n  throw ParseError::\n}\n";
    let mut lsp = Lsp::start();
    open_and_settle(&mut lsp, &uri, source);
    // Cursor right after `ParseError::` (line 6).
    let labels = lsp.completion_labels(&uri, 6, 20);
    assert_eq!(labels, vec!["Empty", "BadDigit"], "{labels:?}");
    // `Char::` is no longer a type path with built-in members.
    let char_labels = lsp.completion_labels_at_extra(&uri, "  Char::");
    assert!(char_labels.is_empty(), "{char_labels:?}");
    let _ = fs::remove_dir_all(&dir);
    lsp.shutdown_and_exit();
}

// Completion context 1: import paths — sibling modules, then their `pub fn`s.
#[test]
fn completes_import_paths() {
    let dir = temp_dir();
    fs::write(
        dir.join("geometry.emel"),
        "module geometry\n\npub fn square(n: Int) -> Int {\n  n * n\n}\n\npub fn area(w: Int, h: Int) -> Int {\n  w * h\n}\n\nfn helper() -> Int {\n  0\n}\n",
    )
    .unwrap();
    let path = dir.join("main.emel");
    let uri = uri_of(&path);
    fs::write(&path, "").unwrap();
    let mut lsp = Lsp::start();

    // First segment: sibling module files.
    let source = "import \n\nfn main() -> Int uses {} {\n  0\n}\n";
    lsp.open(&uri, source);
    lsp.wait_diagnostics(&uri);
    let labels = lsp.completion_labels(&uri, 0, 7);
    assert!(labels.iter().any(|l| l == "geometry"), "{labels:?}");

    // Item segment: the module's public functions only.
    let source = "import geometry.\n\nfn main() -> Int uses {} {\n  0\n}\n";
    lsp.change(&uri, 2, source);
    lsp.wait_diagnostics(&uri);
    let labels = lsp.completion_labels(&uri, 0, 16);
    assert!(labels.iter().any(|l| l == "square"), "{labels:?}");
    assert!(labels.iter().any(|l| l == "area"), "{labels:?}");
    assert!(!labels.iter().any(|l| l == "helper"), "{labels:?}");
    let _ = fs::remove_dir_all(&dir);
    lsp.shutdown_and_exit();
}

// Unknown methods get a MethodNotFound error, not a hang.
#[test]
fn rejects_unknown_requests() {
    let mut lsp = Lsp::start();
    lsp.next_id += 1;
    let id = lsp.next_id;
    lsp.send(
        &json!({"jsonrpc": "2.0", "id": id, "method": "textDocument/definition", "params": {}}),
    );
    let message = lsp.read_message();
    assert_eq!(message["error"]["code"], json!(-32601), "{message}");
    lsp.shutdown_and_exit();
}

// Hover shows the recorded types: parameters and `let` bindings as
// `name: Type` (inference included), references and expressions as the type
// alone, and function names as their full signature.
#[test]
fn hover_params_lets_and_functions() {
    let dir = temp_dir();
    let path = dir.join("main.emel");
    let uri = uri_of(&path);
    fs::write(&path, "").unwrap();
    let source = "fn add(x: Int, y: Int) -> Int uses {} {\n  x + y\n}\n\nfn main() -> Int uses {} {\n  let s = \"a\"\n  let v = Option::Some(1)\n  add(1, 2)\n}\n";
    let mut lsp = Lsp::start();
    open_and_settle(&mut lsp, &uri, source);
    // The parameter's declaration site names the binding.
    assert!(lsp.hover_value(&uri, 0, 7).contains("x: Int"));
    // A reference shows the type alone.
    assert!(lsp.hover_value(&uri, 1, 2).contains("Int"));
    // An operator hovers as the enclosing expression.
    assert!(lsp.hover_value(&uri, 1, 4).contains("Int"));
    // Un-annotated `let`s show their inferred types.
    assert!(lsp.hover_value(&uri, 5, 6).contains("s: String"));
    assert!(lsp.hover_value(&uri, 6, 6).contains("v: Option<Int>"));
    // A function name — definition or call site — shows the signature.
    assert!(
        lsp.hover_value(&uri, 0, 3)
            .contains("fn add(Int, Int) -> Int")
    );
    assert!(
        lsp.hover_value(&uri, 7, 2)
            .contains("fn add(Int, Int) -> Int")
    );
    let _ = fs::remove_dir_all(&dir);
    lsp.shutdown_and_exit();
}

// Positions are UTF-16 code units: hover past a multibyte string literal
// resolves correctly, and hovering nothing yields a null result.
#[test]
fn hover_utf16_positions_and_null() {
    let dir = temp_dir();
    let path = dir.join("main.emel");
    let uri = uri_of(&path);
    fs::write(&path, "").unwrap();
    let source = "fn greet() -> String uses {} {\n  let x = \"こんにちは\"\n  x\n}\n";
    let mut lsp = Lsp::start();
    open_and_settle(&mut lsp, &uri, source);
    assert!(lsp.hover_value(&uri, 1, 6).contains("x: String"));
    // Inside the literal, past the multibyte characters.
    assert!(lsp.hover_value(&uri, 1, 13).contains("String"));
    assert!(lsp.hover_value(&uri, 2, 2).contains("String"));
    // The `fn` keyword has no type.
    assert!(lsp.hover(&uri, 0, 0).is_null());
    let _ = fs::remove_dir_all(&dir);
    lsp.shutdown_and_exit();
}

// A body keeps the entries checked before its first error, and other bodies
// are unaffected — hover keeps working in a broken file (spec 0033).
#[test]
fn hover_survives_type_errors() {
    let dir = temp_dir();
    let path = dir.join("main.emel");
    let uri = uri_of(&path);
    fs::write(&path, "").unwrap();
    let source = "fn broken() -> Int uses {} {\n  let a = 1\n  unknown_name\n}\n\nfn fine(b: Bool) -> Bool uses {} {\n  b\n}\n";
    let mut lsp = Lsp::start();
    lsp.open(&uri, source);
    let diagnostics = lsp.wait_diagnostics(&uri);
    assert!(!diagnostics.is_empty(), "expected an error to be published");
    assert!(lsp.hover_value(&uri, 1, 6).contains("a: Int"));
    assert!(lsp.hover_value(&uri, 5, 8).contains("b: Bool"));
    assert!(lsp.hover_value(&uri, 6, 2).contains("Bool"));
    let _ = fs::remove_dir_all(&dir);
    lsp.shutdown_and_exit();
}

/// Byte offset of a line/character position (ASCII-only test sources).
fn offset_of(text: &str, line: u64, character: u64) -> usize {
    let mut offset = 0;
    for _ in 0..line {
        offset += text[offset..].find('\n').unwrap() + 1;
    }
    offset + character as usize
}

/// Applies one TextEdit to `text`, the way a client would.
fn apply_edit(text: &str, edit: &Value) -> String {
    let start = offset_of(
        text,
        edit["range"]["start"]["line"].as_u64().unwrap(),
        edit["range"]["start"]["character"].as_u64().unwrap(),
    );
    let end = offset_of(
        text,
        edit["range"]["end"]["line"].as_u64().unwrap(),
        edit["range"]["end"]["character"].as_u64().unwrap(),
    );
    format!(
        "{}{}{}",
        &text[..start],
        edit["newText"].as_str().unwrap(),
        &text[end..]
    )
}

// The non-exhaustive-match quickfix appends the missing arms; applying the
// edit clears the diagnostic.
#[test]
fn code_action_fills_missing_match_arms() {
    let dir = temp_dir();
    let path = dir.join("main.emel");
    let uri = uri_of(&path);
    fs::write(&path, "").unwrap();
    let source = "enum Color {\n  Red\n  Green\n}\n\nfn f(c: Color) -> Int uses {} {\n  match c {\n    Red -> 1\n  }\n}\n";
    let mut lsp = Lsp::start();
    lsp.open(&uri, source);
    let diagnostics = lsp.wait_diagnostics(&uri);
    assert_eq!(diagnostics[0]["code"], json!("non-exhaustive-match"));
    let actions = lsp.code_actions(&uri, 6, 4);
    let actions = actions.as_array().unwrap();
    assert_eq!(actions.len(), 1, "{actions:?}");
    assert_eq!(actions[0]["kind"], json!("quickfix"));
    assert!(actions[0]["title"].as_str().unwrap().contains("Green"));
    let edit = &actions[0]["edit"]["changes"][uri.as_str()][0];
    assert!(
        edit["newText"]
            .as_str()
            .unwrap()
            .contains("Color::Green -> panic(\"TODO: handle Green\")"),
        "{edit}"
    );
    let fixed = apply_edit(source, edit);
    lsp.change(&uri, 2, &fixed);
    let diagnostics = lsp.wait_diagnostics(&uri);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let _ = fs::remove_dir_all(&dir);
    lsp.shutdown_and_exit();
}

// Generic enums get instantiated arms, payload variants bind `_` per field,
// and a match with no arms at all inserts after its opening brace.
#[test]
fn code_action_generic_payload_and_empty_match() {
    let dir = temp_dir();
    let path = dir.join("main.emel");
    let uri = uri_of(&path);
    fs::write(&path, "").unwrap();
    let mut lsp = Lsp::start();

    let source = "fn f(o: Option<Int>) -> Int uses {} {\n  match o {\n    Some(v) -> v\n  }\n}\n";
    lsp.open(&uri, source);
    lsp.wait_diagnostics(&uri);
    let actions = lsp.code_actions(&uri, 1, 4);
    let edit = &actions[0]["edit"]["changes"][uri.as_str()][0];
    assert!(
        edit["newText"]
            .as_str()
            .unwrap()
            .contains("Option::None -> panic(\"TODO: handle None\")"),
        "{edit}"
    );
    let fixed = apply_edit(source, edit);
    lsp.change(&uri, 2, &fixed);
    assert!(lsp.wait_diagnostics(&uri).is_empty());

    let source = "fn f(o: Option<Int>) -> Int uses {} {\n  match o {\n    None -> 0\n  }\n}\n";
    lsp.change(&uri, 3, source);
    lsp.wait_diagnostics(&uri);
    let actions = lsp.code_actions(&uri, 1, 4);
    let edit = &actions[0]["edit"]["changes"][uri.as_str()][0];
    assert!(
        edit["newText"]
            .as_str()
            .unwrap()
            .contains("Option::Some(_) -> panic(\"TODO: handle Some\")"),
        "{edit}"
    );
    let fixed = apply_edit(source, edit);
    lsp.change(&uri, 4, &fixed);
    assert!(lsp.wait_diagnostics(&uri).is_empty());

    let source = "enum Color {\n  Red\n}\n\nfn f(c: Color) -> Int uses {} {\n  match c {\n  }\n}\n";
    lsp.change(&uri, 5, source);
    lsp.wait_diagnostics(&uri);
    let actions = lsp.code_actions(&uri, 5, 4);
    let edit = &actions[0]["edit"]["changes"][uri.as_str()][0];
    assert!(
        edit["newText"]
            .as_str()
            .unwrap()
            .contains("Color::Red -> panic(\"TODO: handle Red\")"),
        "{edit}"
    );
    let fixed = apply_edit(source, edit);
    lsp.change(&uri, 6, &fixed);
    assert!(lsp.wait_diagnostics(&uri).is_empty());

    let _ = fs::remove_dir_all(&dir);
    lsp.shutdown_and_exit();
}

// A wildcard arm means exhaustive-by-catch-all: no action. A position outside
// any match yields none either.
#[test]
fn code_action_absent_when_exhaustive() {
    let dir = temp_dir();
    let path = dir.join("main.emel");
    let uri = uri_of(&path);
    fs::write(&path, "").unwrap();
    let source = "fn f(o: Option<Int>) -> Int uses {} {\n  match o {\n    Some(v) -> v\n    _ -> 0\n  }\n}\n";
    let mut lsp = Lsp::start();
    open_and_settle(&mut lsp, &uri, source);
    assert_eq!(lsp.code_actions(&uri, 1, 4), json!([]));
    assert_eq!(lsp.code_actions(&uri, 0, 0), json!([]));
    let _ = fs::remove_dir_all(&dir);
    lsp.shutdown_and_exit();
}

// The incomplete-impl quickfix generates a stub for every missing method —
// the compiler reports only the first — and applying it clears the diagnostic.
#[test]
fn code_action_generates_impl_stubs() {
    let dir = temp_dir();
    let path = dir.join("main.emel");
    let uri = uri_of(&path);
    fs::write(&path, "").unwrap();
    let mut lsp = Lsp::start();

    // An empty impl: the whole scaffold is generated.
    let source = "trait Greet {\n  fn greet(x: Self) -> String\n}\n\nenum Foo {\n  A\n}\n\nimpl Greet for Foo {\n}\n";
    lsp.open(&uri, source);
    let diagnostics = lsp.wait_diagnostics(&uri);
    assert_eq!(diagnostics[0]["code"], json!("incomplete-impl"));
    let actions = lsp.code_actions(&uri, 8, 5);
    let actions = actions.as_array().unwrap();
    assert_eq!(actions.len(), 1, "{actions:?}");
    assert!(actions[0]["title"].as_str().unwrap().contains("greet"));
    let edit = &actions[0]["edit"]["changes"][uri.as_str()][0];
    let new_text = edit["newText"].as_str().unwrap();
    assert!(
        new_text.contains("fn greet(x: Foo) -> String {"),
        "{new_text}"
    );
    assert!(
        new_text.contains("panic(\"TODO: implement greet\")"),
        "{new_text}"
    );
    let fixed = apply_edit(source, edit);
    lsp.change(&uri, 2, &fixed);
    assert!(lsp.wait_diagnostics(&uri).is_empty());

    // A default method and an already-written method are not re-generated;
    // only the truly missing one is stubbed, after the last written method.
    let source = "trait Shape {\n  fn area(s: Self) -> Int\n  fn name(s: Self) -> String\n  fn describe(s: Self) -> String {\n    \"shape\"\n  }\n}\n\nenum Sq {\n  S\n}\n\nimpl Shape for Sq {\n  fn area(s: Sq) -> Int uses {} {\n    4\n  }\n}\n";
    lsp.change(&uri, 3, source);
    lsp.wait_diagnostics(&uri);
    let actions = lsp.code_actions(&uri, 12, 5);
    let actions = actions.as_array().unwrap();
    assert_eq!(actions.len(), 1, "{actions:?}");
    let title = actions[0]["title"].as_str().unwrap();
    assert!(title.contains("name"), "{title}");
    assert!(
        !title.contains("describe") && !title.contains("area"),
        "{title}"
    );
    let edit = &actions[0]["edit"]["changes"][uri.as_str()][0];
    assert!(
        edit["newText"]
            .as_str()
            .unwrap()
            .contains("fn name(s: Sq) -> String {"),
        "{edit}"
    );
    let fixed = apply_edit(source, edit);
    lsp.change(&uri, 4, &fixed);
    assert!(lsp.wait_diagnostics(&uri).is_empty());

    let _ = fs::remove_dir_all(&dir);
    lsp.shutdown_and_exit();
}

// Compiler diagnostics carry their machine-readable code on the wire.
#[test]
fn diagnostics_carry_codes() {
    let dir = temp_dir();
    let path = dir.join("main.emel");
    let uri = uri_of(&path);
    fs::write(&path, "").unwrap();
    let source = "trait Greet {\n  fn greet(x: Self) -> String\n}\n\nenum Foo {\n  A\n}\n\nimpl Greet for Foo {\n}\n";
    let mut lsp = Lsp::start();
    lsp.open(&uri, source);
    let diagnostics = lsp.wait_diagnostics(&uri);
    let incomplete = diagnostics
        .iter()
        .find(|d| d["message"].as_str().unwrap().contains("Incomplete impl"))
        .unwrap();
    assert_eq!(incomplete["code"], json!("incomplete-impl"));
    let _ = fs::remove_dir_all(&dir);
    lsp.shutdown_and_exit();
}
