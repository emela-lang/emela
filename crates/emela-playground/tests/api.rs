//! The JSON the browser reads from `check`, `format` and `compile`.

use emela_playground::{check, compile, format};
use serde_json::Value;

const HELLO: &str = r#"import Std.Console.{Console, SystemConsole}

fn main() {
  with SystemConsole {
    console <- Console
    console.print_line("hi")
  }
}
"#;

fn parse(json: String) -> Value {
    serde_json::from_str(&json).expect("the functions return JSON")
}

#[test]
fn check_accepts_a_program_with_std_imports() {
    let result = parse(check(HELLO));
    assert_eq!(result["ok"], true);
    assert_eq!(result["diagnostics"], "");
}

#[test]
fn check_renders_errors_against_main_emel_without_color() {
    let result = parse(check("fn main() { x }\n"));
    assert_eq!(result["ok"], false);
    let diagnostics = result["diagnostics"].as_str().unwrap();
    assert!(diagnostics.contains("error[E2101]"), "{diagnostics}");
    assert!(diagnostics.contains("main.emel:1:13"), "{diagnostics}");
    assert!(!diagnostics.contains("/playground"), "{diagnostics}");
    assert!(!diagnostics.contains('\u{1b}'), "{diagnostics}");
}

#[test]
fn warnings_alone_keep_ok() {
    let result = parse(check(
        "error Oops\n\nfn main() {\n  dbg(1 escape { Oops -> 0 })\n}\n",
    ));
    assert_eq!(result["ok"], true);
    assert!(
        result["diagnostics"]
            .as_str()
            .unwrap()
            .contains("warning[W3201]"),
        "{result}"
    );
}

#[test]
fn format_returns_the_formatted_source() {
    let result = parse(format("fn main( ){dbg( 1 )}"));
    assert_eq!(result["ok"], true);
    assert_eq!(result["source"], "fn main() { dbg(1) }\n");
}

#[test]
fn format_keeps_the_source_on_a_syntax_error() {
    let result = parse(format("fn main( {"));
    assert_eq!(result["ok"], false);
    assert_eq!(result["source"], "fn main( {");
    assert!(
        result["diagnostics"].as_str().unwrap().contains("error"),
        "{result}"
    );
}

#[test]
fn compile_returns_the_output_files_with_the_entry() {
    let result = parse(compile(HELLO));
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["entry"], "main.mjs");
    let files = result["files"].as_array().unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert!(paths.contains(&"main.mjs"), "{paths:?}");
    assert!(paths.contains(&"emela_runtime.mjs"), "{paths:?}");
    let entry = files.iter().find(|f| f["path"] == "main.mjs").unwrap();
    assert!(
        entry["text"].as_str().unwrap().contains("export { main"),
        "{entry}"
    );
}

#[test]
fn compile_returns_no_files_on_an_error() {
    let result = parse(compile("fn main() { x }\n"));
    assert_eq!(result["ok"], false);
    assert_eq!(result["files"], Value::Array(vec![]));
    assert_eq!(result["entry"], "");
}
