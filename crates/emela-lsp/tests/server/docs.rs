//! Doc comments in hover and completion.

use emela_driver::Typecheck;
use lsp_types as lsp;
use lsp_types::request::{Completion, HoverRequest, Request as _, SignatureHelpRequest};
use serde_json::json;

use super::{Client, MAIN, range, uri};

/// Requests an IDE result at a substring in the main file, keeping documentation fields.
fn doc_query(
    client: &mut Client,
    method: &str,
    text: &str,
    needle: &str,
    within: usize,
) -> serde_json::Value {
    let offset = text.find(needle).unwrap() + within;
    let prefix = &text[..offset];
    let line = prefix.bytes().filter(|&b| b == b'\n').count() as u32;
    let character = prefix.rsplit('\n').next().unwrap().encode_utf16().count() as u32;
    let id = client.request(method, Client::position(MAIN, line, character));
    client.response(&id).response_result.unwrap()
}

#[test]
fn doc_comments_are_markdown_and_follow_unsaved_changes() {
    let text = "## Doubles **値**.\n##\n##     double(2)\nfn double(value: Int) -> Int { value * 2 }\nfn main() -> Int { double(1) }\n";
    let mut client = Client::initialize(
        &[("/app/Pome.toml", ""), (MAIN, text)],
        None,
        Typecheck::default,
    );
    client.open(MAIN, 1, text);
    let doc = "Doubles **値**.\n\n    double(2)";
    let markup = json!({ "kind": "markdown", "value": doc });
    let hover = doc_query(&mut client, HoverRequest::METHOD, text, "double(1)", 0);
    assert_eq!(hover["contents"]["kind"], "markdown");
    assert_eq!(
        hover["contents"]["value"],
        format!("```emela\ndouble: fn(Int) -> Int\n```\n\n---\n\n{doc}")
    );
    let help = doc_query(
        &mut client,
        SignatureHelpRequest::METHOD,
        text,
        "double(1)",
        7,
    );
    assert_eq!(help["signatures"][0]["documentation"], markup);
    // There is no parameter-doc convention; the whole Markdown remains on the signature.
    assert!(help["signatures"][0]["parameters"][0]["documentation"].is_null());
    let items = doc_query(&mut client, Completion::METHOD, text, "double(1)", 0);
    let item = items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["label"] == "double")
        .unwrap();
    assert_eq!(item["documentation"], markup);

    let changed = text.replace("Doubles **値**.", "Updated docs.");
    client.change(MAIN, 2, &changed);
    let help = doc_query(
        &mut client,
        SignatureHelpRequest::METHOD,
        &changed,
        "double(1)",
        7,
    );
    assert_eq!(
        help["signatures"][0]["documentation"]["value"],
        "Updated docs.\n\n    double(2)"
    );
    let no_doc = "# Ordinary comment.\nfn double(value: Int) -> Int { value * 2 }\nfn main() -> Int { double(1) }\n";
    client.change(MAIN, 3, no_doc);
    let hover = doc_query(&mut client, HoverRequest::METHOD, no_doc, "double(1)", 0);
    assert_eq!(
        hover["contents"]["value"],
        "```emela\ndouble: fn(Int) -> Int\n```"
    );
    let items = doc_query(&mut client, Completion::METHOD, no_doc, "double(1)", 0);
    assert!(
        items
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["label"] == "double")
            .unwrap()["documentation"]
            .is_null()
    );
    client.shutdown();
}

#[test]
fn docs_use_definition_spans_across_modules_and_std() {
    let text = "import Alpha\nimport Beta\nimport Std.Duration\nfn main() -> Int {\n  Alpha.same(1)\n  Beta.same(2)\n  Duration.to_seconds(Duration.milliseconds(1))\n  0\n}\n";
    let alpha = "## Alpha's docs.\npub fn same(n: Int) -> Int { n }\n";
    let beta = "## Beta's docs.\npub fn same(n: Int) -> Int { n }\n";
    let mut client = Client::initialize(
        &[
            ("/app/Pome.toml", ""),
            (MAIN, text),
            ("/app/src/alpha.emel", alpha),
            ("/app/src/beta.emel", beta),
        ],
        None,
        Typecheck::default,
    );
    for (needle, name, within, doc) in [
        ("Alpha.same(1)", "same", 6, "Alpha's docs."),
        ("Beta.same(2)", "same", 5, "Beta's docs."),
        (
            "Duration.to_seconds(",
            "to_seconds",
            9,
            "Truncates toward zero.",
        ),
    ] {
        let hover = doc_query(&mut client, HoverRequest::METHOD, text, needle, within);
        assert!(
            hover["contents"]["value"]
                .as_str()
                .unwrap()
                .ends_with(&format!("\n\n---\n\n{doc}")),
            "{hover}"
        );
        let help = doc_query(
            &mut client,
            SignatureHelpRequest::METHOD,
            text,
            needle,
            within + name.len() + 1,
        );
        assert_eq!(
            help["signatures"][0]["documentation"],
            json!({"kind": "markdown", "value": doc})
        );
        let items = doc_query(&mut client, Completion::METHOD, text, needle, within);
        let item = items
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["label"] == name)
            .unwrap();
        assert_eq!(
            item["documentation"],
            json!({"kind": "markdown", "value": doc})
        );
    }
    // Reading the definition uses the overlay too, not the dependency's disk contents.
    client.open(
        "/app/src/alpha.emel",
        1,
        &alpha.replace("Alpha's docs.", "Unsaved docs."),
    );
    let help = doc_query(
        &mut client,
        SignatureHelpRequest::METHOD,
        text,
        "Alpha.same(1)",
        11,
    );
    assert_eq!(
        help["signatures"][0]["documentation"]["value"],
        "Unsaved docs."
    );
    client.shutdown();
}

#[test]
fn docs_coexist_with_highlights_and_completion_based_quickfixes() {
    let text = "## Returns a count.\nfn count(value: Int) -> Int { value }\n## A record.\ntype Point(value: Int)\ntrait Read { fn read(self) -> Int }\n## Implements reading.\nimpl Point: Read {\n  ## Reads the field.\n  fn read(self) { self.value }\n}\nfn main() -> Int { count(1) }\nfn broken() -> Int { cont(2) }\n";
    let mut client = Client::initialize(
        &[("/app/Pome.toml", ""), (MAIN, text)],
        None,
        Typecheck::default,
    );
    let hover = doc_query(&mut client, HoverRequest::METHOD, text, "count(1)", 0);
    assert!(
        hover["contents"]["value"]
            .as_str()
            .unwrap()
            .ends_with("\n\n---\n\nReturns a count.")
    );
    let highlights: Vec<lsp::DocumentHighlight> = serde_json::from_value(doc_query(
        &mut client,
        "textDocument/documentHighlight",
        text,
        "count(1)",
        0,
    ))
    .unwrap();
    assert_eq!(
        highlights.iter().map(|h| h.range).collect::<Vec<_>>(),
        [range((1, 3), (1, 8)), range((10, 19), (10, 24))]
    );
    let items = doc_query(&mut client, Completion::METHOD, text, "cont(2)", 0);
    assert_eq!(
        items
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["label"] == "count")
            .unwrap()["documentation"]["value"],
        "Returns a count."
    );
    let fixes = client.quickfixes(MAIN, (11, 21), (11, 21));
    assert_eq!(fixes.len(), 1);
    let edits = &fixes[0].edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri(MAIN)];
    assert_eq!(edits[0].new_text, "count");

    // Impl methods use their own docs, while local names still resolve through symbol_at.
    let hover = doc_query(
        &mut client,
        HoverRequest::METHOD,
        text,
        "fn read(self) {",
        3,
    );
    assert!(
        hover["contents"]["value"]
            .as_str()
            .unwrap()
            .ends_with("\n\n---\n\nReads the field.")
    );
    let highlights: Vec<lsp::DocumentHighlight> = serde_json::from_value(doc_query(
        &mut client,
        "textDocument/documentHighlight",
        text,
        "fn read(self) {",
        8,
    ))
    .unwrap();
    assert_eq!(
        highlights.iter().map(|h| h.range).collect::<Vec<_>>(),
        [range((8, 10), (8, 14)), range((8, 18), (8, 22))]
    );
    let empty = doc_query(
        &mut client,
        "textDocument/documentHighlight",
        text,
        "impl Point: Read {",
        20,
    );
    assert_eq!(empty, json!([]));
    assert!(
        doc_query(
            &mut client,
            HoverRequest::METHOD,
            text,
            "impl Point: Read {",
            20
        )
        .is_null()
    );
    client.shutdown();
}
