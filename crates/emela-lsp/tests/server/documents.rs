//! Open, changed, closed and watched documents: the diagnostics published for them, file
//! watchers, formatting, and requests the server does not support.

use std::time::Duration;

use emela_driver::ParseOnly;
use lsp_server::Message;
use lsp_types as lsp;
use lsp_types::notification::{DidChangeWatchedFiles, Notification as _};
use lsp_types::request::{RegisterCapability, Request as _};
use serde_json::json;

use super::{Client, Disk, codes, range, uri};

#[test]
fn lex_error_appears_and_clears() {
    let files = [
        ("/app/Pome.toml", ""),
        ("/app/src/main.emel", "const X = 1\n"),
    ];
    let client = Client::initialize(&files, Some("/app"), ParseOnly::new);
    // The contents on disk have no errors, so nothing arrives at first.
    client.open("/app/src/main.emel", 1, "const X = 1\nconst Y = \"\\q\"\n");
    let published = client.diagnostics();
    assert_eq!(published.uri, uri("/app/src/main.emel"));
    assert_eq!(published.version, Some(1));
    assert_eq!(codes(&published), ["E1007"]);
    let diagnostic = &published.diagnostics[0];
    assert_eq!(diagnostic.range, range((1, 11), (1, 13)));
    assert_eq!(diagnostic.severity, Some(lsp::DiagnosticSeverity::ERROR));
    assert_eq!(diagnostic.source.as_deref(), Some("emela"));
    assert_eq!(diagnostic.message, "unknown escape sequence `\\q`");

    client.change("/app/src/main.emel", 2, "const X = 1\nconst Y = 2\n");
    let published = client.diagnostics();
    assert_eq!(published.uri, uri("/app/src/main.emel"));
    assert_eq!(published.version, Some(2));
    assert!(published.diagnostics.is_empty());
    client.shutdown();
}

#[test]
fn columns_are_utf16() {
    // No `Pome.toml`, so this is a standalone file.
    let files = [("/tmp/x/main.emel", "")];
    let client = Client::initialize(&files, None, ParseOnly::new);
    // `あ` and `日本` take one UTF-16 unit per character and `😀` two; in UTF-8, 3 and 4 bytes.
    client.open(
        "/tmp/x/main.emel",
        1,
        "# 日本\nconst S = \"あ😀\\q日本\\q\"\n",
    );
    let published = client.diagnostics();
    assert_eq!(codes(&published), ["E1007", "E1007"]);
    assert_eq!(published.diagnostics[0].range, range((1, 14), (1, 16)));
    assert_eq!(published.diagnostics[1].range, range((1, 18), (1, 20)));
    client.shutdown();
}

#[test]
fn import_errors_go_to_the_importing_file() {
    let files = [
        ("/app/Pome.toml", ""),
        (
            "/app/src/main.emel",
            "import Http.Client\nimport Http.Clinet\n",
        ),
        ("/app/src/http/client.emel", "const GET = 1\n"),
        ("/app/src/util.emel", "const U = 1\n"),
    ];
    // Workspace projects get diagnostics before any file is opened.
    let client = Client::initialize(&files, Some("/app"), ParseOnly::new);
    let published = client.diagnostics();
    assert_eq!(published.uri, uri("/app/src/main.emel"));
    assert_eq!(published.version, None);
    assert_eq!(codes(&published), ["E2004"]);
    assert_eq!(published.diagnostics[0].range, range((1, 7), (1, 18)));
    assert_eq!(
        published.diagnostics[0].message,
        "undefined module `Http.Clinet`"
    );

    // Break the import with unsaved contents of another open file. The diagnostic goes to that
    // file and main does not change.
    client.open(
        "/app/src/util.emel",
        1,
        "import Main\nimport Nope\nconst U = 1\n",
    );
    let published = client.diagnostics_n(2);
    assert_eq!(published[0].uri, uri("/app/src/main.emel"));
    assert_eq!(codes(&published[0]), ["E2004"]);
    assert_eq!(published[1].uri, uri("/app/src/util.emel"));
    assert_eq!(published[1].version, Some(1));
    assert_eq!(codes(&published[1]), ["E2004"]);
    assert_eq!(published[1].diagnostics[0].range, range((1, 7), (1, 11)));

    // A file not yet on disk becomes a module once opened.
    client.open("/app/src/http/clinet.emel", 1, "const GET = 2\n");
    let published = client.diagnostics_n(2);
    // main's error is gone, so it is cleared with an empty array (clearing is sent first).
    assert_eq!(published[0].uri, uri("/app/src/main.emel"));
    assert!(published[0].diagnostics.is_empty());
    assert_eq!(published[1].uri, uri("/app/src/util.emel"));
    assert_eq!(codes(&published[1]), ["E2004"]);
    client.shutdown();
}

/// The server does not go to the network: dependencies it cannot use (here, no `Pome.lock`) are
/// reported on `Pome.toml` (spec 4.7).
#[test]
fn reports_dependencies_that_cannot_be_used() {
    let manifest = "[pome]\nname = \"github.com/me/app\"\nversion = \"0.1.0\"\nemela = \"0.21.0\"\n\n[dependencies]\n\"github.com/acme/x\" = \"v1.0.0\"\n";
    let files = [
        ("/app/Pome.toml", manifest),
        ("/app/src/main.emel", "const X = 1\n"),
    ];
    let client = Client::initialize(&files, Some("/app"), ParseOnly::new);
    let published = client.diagnostics();
    assert_eq!(published.uri, uri("/app/Pome.toml"));
    assert_eq!(codes(&published), ["E2306"]);
    assert!(
        published.diagnostics[0]
            .message
            .contains("emela pome install"),
        "{:?}",
        published.diagnostics
    );
    client.shutdown();
}

/// `[pome.manifest]`: a `[pome].license` that is not an SPDX license expression is a warning at
/// the value in `Pome.toml`.
#[test]
fn reports_a_license_that_is_not_an_spdx_expression() {
    let manifest = "[pome]\nname = \"github.com/me/app\"\nversion = \"0.1.0\"\nemela = \"0.27.0\"\nlicense = \"MIT/Apache\"\n";
    let files = [
        ("/app/Pome.toml", manifest),
        ("/app/src/main.emel", "const X = 1\n"),
    ];
    let client = Client::initialize(&files, Some("/app"), ParseOnly::new);
    let published = client.diagnostics();
    assert_eq!(published.uri, uri("/app/Pome.toml"));
    assert_eq!(codes(&published), ["W2303"]);
    let diagnostic = &published.diagnostics[0];
    assert_eq!(diagnostic.range, range((4, 10), (4, 22)));
    assert_eq!(diagnostic.severity, Some(lsp::DiagnosticSeverity::WARNING));
    client.shutdown();
}

/// `[lsp.project]`, `[pome.manifest]`: the server reads `[experimental]` of the project as the
/// commands do, so an unknown name is W2304 at the key in `Pome.toml`.
#[test]
fn reports_an_unknown_experimental_feature() {
    let manifest = "[pome]\nname = \"github.com/me/app\"\nversion = \"0.1.0\"\nemela = \"0.29.0\"\n\n[experimental]\nfrom-the-future = true\n";
    let files = [
        ("/app/Pome.toml", manifest),
        ("/app/src/main.emel", "const X = 1\n"),
    ];
    let client = Client::initialize(&files, Some("/app"), ParseOnly::new);
    let published = client.diagnostics();
    assert_eq!(published.uri, uri("/app/Pome.toml"));
    assert_eq!(codes(&published), ["W2304"]);
    let diagnostic = &published.diagnostics[0];
    assert_eq!(diagnostic.range, range((6, 0), (6, 15)));
    assert_eq!(diagnostic.severity, Some(lsp::DiagnosticSeverity::WARNING));
    client.shutdown();
}

#[test]
fn close_reverts_to_disk() {
    let files = [
        ("/app/Pome.toml", ""),
        ("/app/src/main.emel", "const X = \"\\q\"\n"),
    ];
    let client = Client::initialize(&files, Some("/app"), ParseOnly::new);
    let published = client.diagnostics();
    assert_eq!(codes(&published), ["E1007"]);
    assert_eq!(published.version, None);

    client.open("/app/src/main.emel", 1, "const X = 1\n");
    let published = client.diagnostics();
    assert!(published.diagnostics.is_empty());
    assert_eq!(published.version, Some(1));

    // Saving does not change the diagnostics (the disk is not written).
    client.save("/app/src/main.emel");
    client.close("/app/src/main.emel");
    let published = client.diagnostics();
    assert_eq!(published.uri, uri("/app/src/main.emel"));
    assert_eq!(published.version, None);
    assert_eq!(codes(&published), ["E1007"]);
    client.shutdown();
}

/// Even if the editor spells the URI differently from the server (here, parentheses are not
/// encoded), the old spelling is cleared first, so the last diagnostics sent for the file remain.
#[test]
fn uri_spelling_changes_clear_first() {
    let files = [
        ("/w/proj (1)/Pome.toml", ""),
        ("/w/proj (1)/src/main.emel", "const X = \"\\q\"\n"),
    ];
    let main = "/w/proj%20(1)/src/main.emel";
    let client = Client::initialize(&files, Some("/w/proj%20(1)"), ParseOnly::new);
    let published = client.diagnostics();
    assert_eq!(
        published.uri.as_str(),
        "file:///w/proj%20%281%29/src/main.emel"
    );
    assert_eq!(codes(&published), ["E1007"]);

    client.open(main, 1, "const X = \"\\q\"\n");
    let cleared = client.diagnostics();
    assert_eq!(
        cleared.uri.as_str(),
        "file:///w/proj%20%281%29/src/main.emel"
    );
    assert!(cleared.diagnostics.is_empty());
    let published = client.diagnostics();
    assert_eq!(published.uri, uri(main));
    assert_eq!(codes(&published), ["E1007"]);

    client.close(main);
    let cleared = client.diagnostics();
    assert_eq!(cleared.uri, uri(main));
    assert!(cleared.diagnostics.is_empty());
    let published = client.diagnostics();
    assert_eq!(
        published.uri.as_str(),
        "file:///w/proj%20%281%29/src/main.emel"
    );
    assert_eq!(codes(&published), ["E1007"]);
    client.shutdown();
}

#[test]
fn closing_a_single_file_clears_it() {
    let files = [("/tmp/x/main.emel", "")];
    let client = Client::initialize(&files, None, ParseOnly::new);
    client.open("/tmp/x/main.emel", 1, "const X = \"\\q\"\n");
    assert_eq!(codes(&client.diagnostics()), ["E1007"]);
    client.close("/tmp/x/main.emel");
    let published = client.diagnostics();
    assert_eq!(published.uri, uri("/tmp/x/main.emel"));
    assert!(published.diagnostics.is_empty());
    client.shutdown();
}

/// Files changed on disk without the editor (a checkout, another tool) are checked again when
/// the editor reports them. An open file keeps its unsaved contents, so a change to it on disk is
/// ignored. A deleted file's diagnostics are cleared.
#[test]
fn watched_file_changes_recheck_the_project() {
    use lsp::FileChangeType as Change;
    let disk = Disk::new(&[
        ("/app/Pome.toml", ""),
        ("/app/src/main.emel", "const X = 1\n"),
        ("/app/src/util.emel", "const U = \"\\q\"\n"),
    ]);
    // The disk is changed only after a publish, so the server has read it before the change.
    let client = Client::initialize_on(disk.clone(), Some("/app"), json!({}), ParseOnly::new);
    assert_eq!(codes(&client.diagnostics()), ["E1007"]);

    // The open buffer wins over the disk, so this change on disk publishes nothing.
    client.open("/app/src/main.emel", 1, "const X = 1\n");
    assert_eq!(codes(&client.diagnostics()), ["E1007"]);
    disk.write("/app/src/main.emel", "const X = \"\\q\"\n");
    client.watched(&[("/app/src/main.emel", Change::CHANGED)]);

    // A change to `Pome.toml` counts too: dependencies without a lock are reported on it.
    disk.write(
        "/app/Pome.toml",
        "[pome]\nname = \"github.com/me/app\"\nversion = \"0.1.0\"\nemela = \"0.21.0\"\n\n[dependencies]\n\"github.com/acme/x\" = \"v1.0.0\"\n",
    );
    client.watched(&[("/app/Pome.toml", Change::CHANGED)]);
    let published = client.diagnostics_n(2);
    assert_eq!(published[0].uri, uri("/app/Pome.toml"));
    assert_eq!(codes(&published[0]), ["E2306"]);
    assert_eq!(published[1].uri, uri("/app/src/util.emel"));
    assert_eq!(codes(&published[1]), ["E1007"]);

    disk.write("/app/Pome.toml", "");
    disk.delete("/app/src/util.emel");
    client.watched(&[
        ("/app/Pome.toml", Change::CHANGED),
        ("/app/src/util.emel", Change::DELETED),
    ]);
    let cleared = client.diagnostics_n(2);
    assert_eq!(cleared[0].uri, uri("/app/Pome.toml"));
    assert!(cleared[0].diagnostics.is_empty());
    assert_eq!(cleared[1].uri, uri("/app/src/util.emel"));
    assert!(cleared[1].diagnostics.is_empty());

    disk.write("/app/src/new.emel", "const N = \"\\q\"\n");
    client.watched(&[("/app/src/new.emel", Change::CREATED)]);
    let published = client.diagnostics();
    assert_eq!(published.uri, uri("/app/src/new.emel"));
    assert_eq!(published.version, None);
    assert_eq!(codes(&published), ["E1007"]);

    // A file that cannot change a check is ignored.
    client.watched(&[("/app/README.md", Change::CHANGED)]);
    client.shutdown_with_nothing_published();
}

/// A file that an open single file imports is checked again when it changes on disk.
#[test]
fn watched_file_changes_recheck_single_file_imports() {
    let disk = Disk::new(&[
        ("/tmp/x/main.emel", "import Util\n"),
        ("/tmp/x/util.emel", "const U = \"\\q\"\n"),
    ]);
    let client = Client::initialize_on(disk.clone(), None, json!({}), ParseOnly::new);
    client.open("/tmp/x/main.emel", 1, "import Util\n");
    let published = client.diagnostics();
    assert_eq!(published.uri, uri("/tmp/x/util.emel"));
    assert_eq!(codes(&published), ["E1007"]);

    disk.write("/tmp/x/util.emel", "const U = 1\n");
    client.watched(&[("/tmp/x/util.emel", lsp::FileChangeType::CHANGED)]);
    let published = client.diagnostics();
    assert_eq!(published.uri, uri("/tmp/x/util.emel"));
    assert!(published.diagnostics.is_empty());
    client.shutdown();
}

/// A client that can register `workspace/didChangeWatchedFiles` dynamically is asked to watch the
/// files that affect a check, and its answer does not disturb the server.
#[test]
fn registers_file_watchers_when_the_client_can() {
    let capabilities = json!({
        "workspace": { "didChangeWatchedFiles": { "dynamicRegistration": true } },
    });
    let disk = Disk::new(&[
        ("/app/Pome.toml", ""),
        ("/app/src/main.emel", "const X = \"\\q\"\n"),
    ]);
    let client = Client::initialize_on(disk, Some("/app"), capabilities, ParseOnly::new);
    let request = match client.recv() {
        Message::Request(request) if request.method == RegisterCapability::METHOD => request,
        other => panic!("not registerCapability: {other:?}"),
    };
    let params: lsp::RegistrationParams = serde_json::from_value(request.params).unwrap();
    assert_eq!(params.registrations.len(), 1);
    let registration = &params.registrations[0];
    assert_eq!(registration.method, DidChangeWatchedFiles::METHOD);
    let options: lsp::DidChangeWatchedFilesRegistrationOptions =
        serde_json::from_value(registration.register_options.clone().unwrap()).unwrap();
    let globs: Vec<_> = options
        .watchers
        .iter()
        .map(|watcher| match &watcher.glob_pattern {
            lsp::GlobPattern::String(glob) => (glob.as_str(), watcher.kind),
            other => panic!("not a string glob: {other:?}"),
        })
        .collect();
    assert_eq!(
        globs,
        [
            ("**/*.emel", None),
            ("**/Pome.toml", None),
            ("**/Pome.lock", None),
        ]
    );
    client.send(lsp_server::Response::new_ok(request.id, serde_json::Value::Null).into());
    assert_eq!(codes(&client.diagnostics()), ["E1007"]);
    client.shutdown();
}

#[test]
fn consecutive_changes_publish_only_the_latest() {
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", "")];
    let (mut client, server) = Client::new();
    // Queue everything before starting the server. Pending edits are analyzed once together.
    client.send_initialize(Some("/app"));
    client.open("/app/src/main.emel", 1, "const X = $\n");
    client.change("/app/src/main.emel", 2, "const A = 3px\n");
    client.change("/app/src/main.emel", 3, "const B = 1\nconst C = \"\n");
    client.start(server, &files, ParseOnly::new);
    assert!(matches!(client.recv(), Message::Response(_)));
    let published = client.diagnostics();
    assert_eq!(published.version, Some(3));
    assert_eq!(codes(&published), ["E1001"]);
    assert_eq!(published.diagnostics[0].range, range((1, 10), (1, 11)));
    // Diagnostics for the old version never arrive (the shutdown response comes next).
    client.shutdown_with_nothing_published();
}

#[test]
fn file_outside_the_source_root() {
    let files = [
        ("/app/Pome.toml", ""),
        ("/app/src/main.emel", "const X = 1\n"),
        ("/app/scripts/x.emel", ""),
    ];
    let client = Client::initialize(&files, Some("/app"), ParseOnly::new);
    client.open("/app/scripts/x.emel", 1, "const Y = 1\n");
    let published = client.diagnostics();
    assert_eq!(published.uri, uri("/app/scripts/x.emel"));
    assert_eq!(codes(&published), ["E2006"]);
    assert_eq!(published.diagnostics[0].range, range((0, 0), (0, 0)));
    assert!(
        published.diagnostics[0]
            .message
            .contains("note: source files of a project"),
        "{}",
        published.diagnostics[0].message
    );
    client.shutdown();
}

#[test]
fn unsupported_requests_get_an_error() {
    let mut client = Client::initialize(&[("/tmp/main.emel", "")], None, ParseOnly::new);
    let id = client.request("textDocument/codeLens", json!({}));
    match client.recv() {
        Message::Response(response) => {
            assert_eq!(response.id, id);
            assert_eq!(
                response.response_result.unwrap_err().code,
                lsp_server::ErrorCode::MethodNotFound as i32
            );
        }
        other => panic!("{other:?}"),
    }
    client.shutdown();
}

#[test]
fn formatting_replaces_the_whole_document() {
    let files = [
        ("/app/Pome.toml", ""),
        ("/app/src/main.emel", "fn main() { 1 }\n"),
        ("/app/src/util.emel", "fn f( ) {1}\n"),
    ];
    let mut client = Client::initialize(&files, Some("/app"), ParseOnly::new);
    // Formatting is announced in the capabilities.
    assert_eq!(
        emela_lsp::capabilities().document_formatting_provider,
        Some(lsp::OneOf::Left(true))
    );
    // An open document formats its unsaved contents. The range covers the whole document
    // (through the final newline).
    client.open("/app/src/main.emel", 1, "fn main( ) {\n  \"あ\"}");
    let edits = client.format("/app/src/main.emel").unwrap();
    assert_eq!(
        edits,
        vec![lsp::TextEdit {
            range: range((0, 0), (1, 6)),
            new_text: "fn main() {\n  \"あ\"\n}\n".to_owned(),
        }]
    );
    // No edits if already formatted.
    client.change("/app/src/main.emel", 2, "fn main() { 1 }\n");
    assert_eq!(client.format("/app/src/main.emel"), Some(vec![]));
    // The contents on disk if not open.
    let edits = client.format("/app/src/util.emel").unwrap();
    assert_eq!(edits[0].range, range((0, 0), (1, 0)));
    assert_eq!(edits[0].new_text, "fn f() { 1 }\n");
    // null on syntax errors.
    client.change("/app/src/main.emel", 3, "fn main( {\n");
    assert_eq!(client.format("/app/src/main.emel"), None);
    // null for unreadable files too.
    assert_eq!(client.format("/app/src/nowhere.emel"), None);
    // Read the diagnostics that have not arrived yet before finishing.
    while let Ok(message) = client
        .connection
        .receiver
        .recv_timeout(Duration::from_millis(200))
    {
        assert!(matches!(message, Message::Notification(_)), "{message:?}");
    }
    client.shutdown();
}
