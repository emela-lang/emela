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

use std::path::{Path, PathBuf};

use emela_driver::{
    Analysis, Backend, CoreJs, Input, Lower, MemoryFiles, Target, format_source, render,
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

fn json(value: &impl Serialize) -> String {
    serde_json::to_string(value).expect("plain strings and booleans serialize")
}
