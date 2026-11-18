//! The language server driven one message at a time, as the editor in the browser drives it.

use emela_playground::LspSession;
use serde_json::{Value, json};

const URI: &str = "file:///playground/main.emel";

fn send(session: &mut LspSession, message: Value) -> Vec<Value> {
    let replies: Value = serde_json::from_str(&session.send(&message.to_string()))
        .expect("send returns a JSON array");
    replies.as_array().expect("an array").clone()
}

/// Initializes a session and opens `source`; returns what opening it sent too.
fn opened(source: &str) -> (LspSession, Vec<Value>) {
    let mut session = LspSession::new();
    let replies = send(
        &mut session,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "capabilities": {} } }),
    );
    assert_eq!(replies.len(), 1, "{replies:?}");
    assert!(replies[0]["result"]["capabilities"]["completionProvider"].is_object());
    send(
        &mut session,
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
    );
    let replies = send(
        &mut session,
        json!({ "jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
            "textDocument": { "uri": URI, "languageId": "emela", "version": 1, "text": source }
        } }),
    );
    (session, replies)
}

fn initialized(source: &str) -> LspSession {
    opened(source).0
}

fn position(source: &str, needle: &str) -> Value {
    let offset = source.find(needle).expect("the needle is in the source");
    let line = source[..offset].matches('\n').count();
    let character = offset - source[..offset].rfind('\n').map_or(0, |i| i + 1);
    json!({ "line": line, "character": character })
}

#[test]
fn a_request_before_initialize_is_refused() {
    let mut session = LspSession::new();
    let replies = send(
        &mut session,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "textDocument/hover", "params": {} }),
    );
    assert_eq!(replies[0]["error"]["code"], -32002, "{replies:?}");
}

#[test]
fn opening_and_changing_the_file_publishes_its_diagnostics() {
    let (mut session, replies) = opened("fn main() { x }\n");
    assert_eq!(replies.len(), 1, "{replies:?}");
    assert_eq!(replies[0]["method"], "textDocument/publishDiagnostics");
    let replies = send(
        &mut session,
        json!({ "jsonrpc": "2.0", "method": "textDocument/didChange", "params": {
            "textDocument": { "uri": URI, "version": 2 },
            "contentChanges": [{ "text": "fn main() { y }\n" }]
        } }),
    );
    assert_eq!(replies.len(), 1, "{replies:?}");
    let params = &replies[0]["params"];
    assert_eq!(params["uri"], URI);
    let message = params["diagnostics"][0]["message"].as_str().unwrap();
    assert!(message.contains("`y`"), "{message}");
}

#[test]
fn hover_shows_the_type() {
    let source = "fn twice(n: Int) -> Int { n * 2 }\n\nfn main() { dbg(twice(2)) }\n";
    let mut session = initialized(source);
    let replies = send(
        &mut session,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "textDocument/hover", "params": {
            "textDocument": { "uri": URI }, "position": position(source, "twice(2)")
        } }),
    );
    assert_eq!(replies[0]["id"], 2);
    let hover = replies[0]["result"]["contents"].to_string();
    assert!(hover.contains("Int"), "{hover}");
}

#[test]
fn completion_lists_the_functions_of_a_module() {
    let source = "fn main() { dbg(List.) }\n";
    let mut session = initialized(source);
    let mut at = position(source, "List.");
    at["character"] = json!(at["character"].as_u64().unwrap() + 5);
    let replies = send(
        &mut session,
        json!({ "jsonrpc": "2.0", "id": 3, "method": "textDocument/completion", "params": {
            "textDocument": { "uri": URI }, "position": at
        } }),
    );
    let result = &replies[0]["result"];
    let items = result.get("items").unwrap_or(result).as_array().unwrap();
    assert!(items.iter().any(|item| item["label"] == "map"), "{items:?}");
}

#[test]
fn shutdown_is_answered_without_waiting_for_exit() {
    let mut session = initialized("fn main() {}\n");
    let replies = send(
        &mut session,
        json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown" }),
    );
    assert_eq!(
        replies,
        vec![json!({ "jsonrpc": "2.0", "id": 4, "result": null })]
    );
}

#[test]
fn a_message_that_is_not_json_rpc_gets_nothing() {
    let mut session = LspSession::new();
    assert_eq!(session.send("not json"), "[]");
}
