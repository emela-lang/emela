//! Grapheme clusters (UAX #29, Unicode 17.0.0) in the WASM runtime.
//!
//! The runtime tables come from `data/unicode/17.0.0/`; these tests run the whole
//! `GraphemeBreakTest.txt` through a generated program, compare representative strings with
//! `Intl.Segmenter` on Node 24 (Unicode 17.0), and check `String.length`, `String.chars` and
//! `String.split(s, "")`. Wasmtime and node are optional, like in the other run tests.

mod common;
mod grapheme_break_test;

use std::process::{Command, Stdio};

use common::*;

/// The unit separator between the graphemes in the generated expected strings. The samples do
/// not contain it.
const SEPARATOR: char = '\u{1f}';

/// A string literal for an Emela source: everything outside printable ASCII is escaped.
fn literal(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '#' => out.push_str("\\#"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ' '..='~' => out.push(c),
            _ => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
        }
    }
    out.push('"');
    out
}

/// The checks shared by the tests: each case is `(string, graphemes)`. `String.length`,
/// `String.chars` and `String.split(s, "")` must agree with the graphemes; a failing case prints
/// its index to stderr.
fn check_program(cases: &[(String, Vec<String>)]) -> String {
    let mut src = String::from(concat!(
        "fn check(n: Int, s: String, want: String, len: Int) {\n",
        "  got = String.join(String.chars(s), \"\\u{1f}\")\n",
        "  split = String.join(String.split(s, \"\"), \"\\u{1f}\")\n",
        "  if got == want && split == want && String.length(s) == len {\n",
        "    ()\n",
        "  } else {\n",
        "    dbg(n)\n",
        "    ()\n",
        "  }\n",
        "}\n\n",
        "fn main() {\n",
    ));
    for (i, (s, graphemes)) in cases.iter().enumerate() {
        let want = graphemes.join(&SEPARATOR.to_string());
        src.push_str(&format!(
            "  check({i}, {}, {}, {})\n",
            literal(s),
            literal(&want),
            graphemes.len()
        ));
    }
    src.push_str("}\n");
    src
}

/// Runs the generated program; on failure the index of the first bad case is in stderr.
fn check(cases: &[(String, Vec<String>)]) -> Result<(), String> {
    let Some(run) = run("unicode-cases", &check_program(cases)) else {
        return Ok(());
    };
    if run.code != 0 {
        return Err(format!(
            "the program exited with {}: {}",
            run.code, run.stderr
        ));
    }
    if run.stderr.is_empty() {
        return Ok(());
    }
    let index: usize = run
        .stderr
        .lines()
        .next()
        .and_then(|line| line.trim().parse().ok())
        .unwrap_or(usize::MAX);
    let detail = cases
        .get(index)
        .map(|(s, graphemes)| {
            format!("case {index} {s:?}: the runtime disagrees with the expected {graphemes:?}")
        })
        .unwrap_or_else(|| format!("the program printed {:?}", run.stderr));
    Err(detail)
}

#[test]
fn official_grapheme_break_test_on_wasm() {
    let cases: Vec<(String, Vec<String>)> = grapheme_break_test::cases()
        .into_iter()
        .map(|graphemes| (graphemes.concat(), graphemes))
        .collect();
    if let Err(e) = check(&cases) {
        panic!("{e}");
    }
}

/// The strings the JS comparison uses: emoji, ZWJ sequences, flags, Indic conjuncts, combining
/// marks, CRLF and Hangul.
const SAMPLES: &[&str] = &[
    "",
    "a",
    "abc",
    "👍",
    "👍🏽",
    "e\u{301}",
    "a\u{301}\u{302}b",
    "👨\u{200d}👩\u{200d}👧\u{200d}👦",
    "🏳\u{fe0f}\u{200d}🌈",
    "🇯🇵",
    "🇯🇵🇺🇸🇫🇷",
    "\u{1f1e6}",
    "\u{1f3f4}\u{e0067}\u{e0062}\u{e0073}\u{e0063}\u{e0074}\u{e007f}",
    "क्ष",
    "क्षत्रिय",
    "क्\u{200d}ष",
    "हिन्दी",
    "한글",
    "\u{1100}\u{1161}\u{11a8}",
    "\u{ac00}\u{ac01}",
    "a\r\nb",
    "\r\n",
    "\r\n\r\n",
    "日本語",
    "e\u{301}\u{200d}e",
    "\u{0}\u{1}\u{200d}",
];

fn node() -> Option<&'static str> {
    static NODE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    NODE.get_or_init(|| {
        let node = std::env::var("EMELA_NODE")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "node".to_owned());
        let output = Command::new(&node).arg("--version").output().ok()?;
        output.status.success().then_some(node)
    })
    .as_deref()
}

/// Segments the samples with `Intl.Segmenter` and reads back `(string, graphemes)`.
///
/// The samples go to node on stdin as lines of hex code points (a sample can contain a NUL or a
/// newline), and the graphemes come back the same way: `;` between graphemes, `,` between the
/// code points of one grapheme.
fn segment_with_node(samples: &[&str]) -> Option<Vec<(String, Vec<String>)>> {
    let node = node()?;
    let script = r#"
const fs = require("fs");
const segmenter = new Intl.Segmenter(undefined, { granularity: "grapheme" });
const lines = fs.readFileSync(0, "utf8").split("\n");
lines.pop();
const out = lines.map((line) => {
  const s = String.fromCodePoint(...(line === "" ? [] : line.split(",").map((h) => parseInt(h, 16))));
  return [...segmenter.segment(s)]
    .map((part) => [...part.segment].map((c) => c.codePointAt(0).toString(16)).join(","))
    .join(";");
});
console.error(process.versions.unicode);
process.stdout.write(out.join("\n"));
"#;
    let hex = |s: &str| {
        s.chars()
            .map(|c| format!("{:x}", c as u32))
            .collect::<Vec<_>>()
            .join(",")
    };
    let mut input = samples
        .iter()
        .map(|s| hex(s))
        .collect::<Vec<_>>()
        .join("\n");
    input.push('\n');
    let mut child = Command::new(node)
        .arg("-e")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    {
        use std::io::Write;
        child.stdin.take()?.write_all(input.as_bytes()).ok()?;
    }
    let output = child.wait_with_output().ok()?;
    assert!(
        output.status.success(),
        "node failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let unicode = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !unicode.starts_with("17.") {
        eprintln!("skipping: node uses Unicode {unicode}, not 17.0 (Intl.Segmenter may differ)");
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), samples.len(), "node printed fewer lines");
    Some(
        lines
            .iter()
            .zip(samples)
            .map(|(line, &sample)| {
                let graphemes = line
                    .split(';')
                    .filter(|part| !part.is_empty())
                    .map(|part| {
                        part.split(',')
                            .map(|c| {
                                let cp = u32::from_str_radix(c, 16).expect("a code point");
                                char::from_u32(cp).expect("a scalar value")
                            })
                            .collect::<String>()
                    })
                    .collect();
                (sample.to_owned(), graphemes)
            })
            .collect(),
    )
}

#[test]
fn intl_segmenter_agrees_on_wasm() {
    let Some(cases) = segment_with_node(SAMPLES) else {
        return;
    };
    if let Err(e) = check(&cases) {
        let table: Vec<String> = cases
            .iter()
            .map(|(s, g)| format!("  {s:?} -> {g:?}"))
            .collect();
        panic!("{e}\nIntl.Segmenter said:\n{}", table.join("\n"));
    }
}

#[test]
fn string_builtins_use_graphemes() {
    let Some(out) = run(
        "unicode-builtins",
        r#"
fn main() {
  dbg(String.length("👍🏽"))
  dbg(String.chars("e\u{301}x"))
  dbg(String.split("👨\u{200d}👩\u{200d}👧", ""))
  dbg(String.split("🇯🇵🇺🇸", ""))
  dbg(String.length("क्ष"))
  dbg(String.chars("\r\n"))
  dbg(String.split("", ""))
  dbg(String.length(""))
}
"#,
    ) else {
        return;
    };
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(
        out.stderr,
        "1\n[\"e\u{301}\", \"x\"]\n[\"👨\u{200d}👩\u{200d}👧\"]\n\
         [\"🇯🇵\", \"🇺🇸\"]\n1\n[\"\\r\\n\"]\n[]\n0\n"
    );
}
