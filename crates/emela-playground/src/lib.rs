//! The compiler for the browser: checks, formats and compiles one source file to JS, without a
//! file system or a process.
//!
//! Built for `wasm32-unknown-unknown` with wasm-bindgen; the docs site runs it in a Web Worker
//! and runs the JS it returns in another. The source is one file, `main.emel`, with no
//! `Pome.toml`: only the bundled core and std are importable, as for `emela run main.emel`
//! outside a project.
//!
//! Every function takes the source text and returns a JSON object (as a string), so the JS side
//! needs no generated types:
//!
//! - [`check`]: `{ "ok": bool, "diagnostics": string }`
//! - [`format`]: `{ "ok": bool, "source": string, "diagnostics": string }`
//! - [`compile`]: `{ "ok": bool, "diagnostics": string, "entry": string, "files": [{ "path", "text" }] }`
//!
//! `diagnostics` is rendered as `emela check` prints it without color, with the file shown as
//! `main.emel`. `ok` is false when there is an error; warnings alone keep it true.
//!
//! [`LspSession`] is the language server of `emela lsp`, driven one message at a time: the editor
//! sends it LSP messages as JSON text and gets back what the server answers. The file an editor
//! opens is `file:///playground/main.emel`; nothing is on the file system until it is opened.

use std::path::{Path, PathBuf};

use emela_driver::{
    Analysis, Backend, CoreJs, Input, Lower, MemoryFiles, Target, Typecheck, format_source, render,
};
use serde::Serialize;
use wasm_bindgen::prelude::wasm_bindgen;

/// Where the source is put in the in-memory file system. Diagnostics show it relative to its
/// directory, as `main.emel`.
const ENTRY: &str = "/playground/main.emel";

#[derive(Serialize)]
struct Checked {
    ok: bool,
    diagnostics: String,
}

#[derive(Serialize)]
struct Formatted {
    ok: bool,
    source: String,
    diagnostics: String,
}

#[derive(Serialize)]
struct Compiled {
    ok: bool,
    diagnostics: String,
    entry: String,
    files: Vec<File>,
}

#[derive(Serialize)]
struct File {
    path: String,
    text: String,
}

/// Reports the diagnostics of `source`, as `emela check main.emel`.
#[wasm_bindgen]
pub fn check(source: &str) -> String {
    let (analysis, _) = analyze(source);
    json(&Checked {
        ok: !analysis.has_errors(),
        diagnostics: render(&analysis.diagnostics, &analysis.sources, false),
    })
}

/// Formats `source`, as `emela fmt`. On a syntax error, `source` is returned unchanged.
#[wasm_bindgen]
pub fn format(source: &str) -> String {
    let result = match format_source(Path::new("main.emel"), source) {
        Ok(formatted) => Formatted {
            ok: true,
            source: formatted.unwrap_or_else(|| source.to_owned()),
            diagnostics: String::new(),
        },
        Err(failure) => Formatted {
            ok: false,
            source: source.to_owned(),
            diagnostics: render(&failure.diagnostics, &failure.sources, false),
        },
    };
    json(&result)
}

/// Compiles `source` to JS, as `emela build main.emel`, and returns the files of the output
/// directory instead of writing them. `entry` is the module that exports `main`.
#[wasm_bindgen]
pub fn compile(source: &str) -> String {
    let (mut analysis, program) = analyze(source);
    let mut files = Vec::new();
    let mut entry = String::new();
    if !analysis.has_errors()
        && let Some(program) = program
    {
        match CoreJs.emit(&program, &analysis) {
            Ok(output) => {
                entry = output.entry.display().to_string();
                files = output
                    .files
                    .iter()
                    .map(|file| File {
                        path: file.path.display().to_string(),
                        text: file.text().to_owned(),
                    })
                    .collect();
            }
            Err(diagnostics) => analysis.diagnostics.extend(diagnostics),
        }
    }
    json(&Compiled {
        ok: !analysis.has_errors(),
        diagnostics: render(&analysis.diagnostics, &analysis.sources, false),
        entry,
        files,
    })
}

/// Checks `source` as the single file `main.emel` for the JS target.
fn analyze(source: &str) -> (Analysis, Option<emela_driver::CoreModule>) {
    let fs = MemoryFiles::new([(ENTRY, source)]);
    let input = Input::resolve(&fs, &PathBuf::from(ENTRY))
        .expect("the entry was just put in the in-memory file system");
    emela_driver::check(&fs, &input, &mut Lower::default().with_target(Target::Js))
}

/// The language server, as `emela lsp` runs it, on an empty in-memory file system.
#[wasm_bindgen]
pub struct LspSession {
    session: emela_lsp::Session<MemoryFiles, fn() -> Typecheck, Typecheck>,
}

#[wasm_bindgen]
impl LspSession {
    #[wasm_bindgen(constructor)]
    #[allow(clippy::new_without_default)] // A JS class: `new LspSession()`.
    pub fn new() -> LspSession {
        let files: [(&str, &str); 0] = [];
        LspSession {
            session: emela_lsp::Session::new(MemoryFiles::new(files), Typecheck::default),
        }
    }

    /// Handles one LSP message (a JSON-RPC object as text) and returns the messages the server
    /// sends because of it, as a JSON array of the same objects. A message that is not valid
    /// JSON-RPC is answered with nothing.
    pub fn send(&mut self, message: &str) -> String {
        let replies = match serde_json::from_str::<lsp_server::Message>(message) {
            Ok(message) => self.session.handle(message),
            Err(_) => Vec::new(),
        };
        // `Message` leaves out `"jsonrpc": "2.0"`, which lsp-server adds only when it writes to a
        // stream.
        let replies: Vec<serde_json::Value> = replies
            .into_iter()
            .map(|reply| {
                let mut value = serde_json::to_value(reply).expect("LSP messages serialize");
                if let Some(object) = value.as_object_mut() {
                    object.insert("jsonrpc".to_owned(), "2.0".into());
                }
                value
            })
            .collect();
        serde_json::to_string(&replies).expect("JSON values serialize")
    }
}

fn json(value: &impl Serialize) -> String {
    serde_json::to_string(value).expect("plain strings and booleans serialize")
}
