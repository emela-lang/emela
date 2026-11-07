//! Formatter tests. Snapshots the output for `test_data/*.emel` and checks, for every input:
//!
//! - idempotence: `format(format(x)) == format(x)`
//! - meaning is preserved: the tokens match, ignoring trivia, trailing commas and newlines at
//!   block edges
//! - no comment is dropped: the comment texts match
//!
//! Line breaking depends on the width, so the properties are checked at several widths.

use std::path::Path;

use emela_fmt::{FormatError, comment_texts, format, format_with_width, normalized_tokens};
use emela_syntax::parse;
use proptest::prelude::*;

const WIDTHS: &[usize] = &[100, 80, 60, 40, 30, 20, 10, 1];

fn inputs() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("test_data");
    let mut inputs: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "emel"))
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read_to_string(&p).unwrap())
        })
        .collect();
    inputs.sort();
    assert!(!inputs.is_empty());
    inputs
}

fn check(name: &str, src: &str, width: usize) -> String {
    let out = format_with_width(src, width)
        .unwrap_or_else(|e| panic!("cannot format {name} (width {width}): {e}"));
    let again = format_with_width(&out, width).unwrap();
    assert_eq!(again, out, "{name} (width {width}) is not idempotent");
    let before = parse(src).syntax();
    let after = parse(&out).syntax();
    assert_eq!(
        normalized_tokens(&before),
        normalized_tokens(&after),
        "{name} (width {width}) changed the tokens"
    );
    assert_eq!(
        comment_texts(&before),
        comment_texts(&after),
        "{name} (width {width}) changed the comments"
    );
    out
}

#[test]
fn snapshots() {
    insta::glob!("../test_data", "*.emel", |path| {
        let src = std::fs::read_to_string(path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy();
        insta::assert_snapshot!(check(&name, &src, 100));
    });
}

#[test]
fn narrow_snapshots() {
    for name in [
        "long.emel",
        "comments.emel",
        "decls.emel",
        "postfix_match.emel",
        "handler_init_args.emel",
    ] {
        let (_, src) = inputs().into_iter().find(|(n, _)| n == name).unwrap();
        insta::assert_snapshot!(format!("{name}_40"), check(name, &src, 40));
    }
}

#[test]
fn すべての幅で冪等で意味を変えない() {
    for (name, src) in inputs() {
        for &width in WIDTHS {
            check(&name, &src, width);
        }
    }
}

#[test]
fn 構文エラーがあれば整形しない() {
    let err = format("fn f( {\n").unwrap_err();
    assert!(
        matches!(err, FormatError::Syntax(ref d) if !d.is_empty()),
        "{err:?}"
    );
    assert!(matches!(
        format("fn f(x: Int) -> Int\n"),
        Err(FormatError::Syntax(_))
    ));
}

#[test]
fn 空のファイルとコメントだけのファイル() {
    assert_eq!(format("").unwrap(), "");
    assert_eq!(format("\n\n  \n").unwrap(), "");
    assert_eq!(format("\n# a\n\n\n# b\n").unwrap(), "# a\n\n# b\n");
}

#[test]
fn 改行は_lf_にそろえる() {
    assert_eq!(
        format("fn f() {\r\n  a\r\n}\r\n").unwrap(),
        "fn f() {\n  a\n}\n"
    );
    // `\r\n` and `\n` count as the same newline even where statement separators are compared.
    assert_eq!(
        format("fn a() {\r\n  x = 1\r\n  y = 2\r\n}\r\n\r\nfn b() {}\r\n").unwrap(),
        "fn a() {\n  x = 1\n  y = 2\n}\n\nfn b() {}\n"
    );
}

#[test]
fn 複数行の文字列の中身は変えない() {
    // Indentation and trailing whitespace are part of the value; they stay put even when the
    // enclosing block's indentation changes.
    let src = "fn a() {\r\n      s = \"\"\"\r\n  x  \r\n\r\n    y\r\n  \"\"\"\r\n}\r\n";
    let expected = "fn a() {\n  s = \"\"\"\n  x  \n\n    y\n  \"\"\"\n}\n";
    for width in [100, 10] {
        assert_eq!(
            format_with_width(src, width).unwrap(),
            expected,
            "width {width}"
        );
    }
}

#[test]
fn 中身のないブロックの行末のコメントの後に空行を入れない() {
    assert_eq!(format("fn a() { # c\n}\n").unwrap(), "fn a() { # c\n}\n");
    assert_eq!(
        format("fn a() {\n  f(|y| { # d\n  })\n}\n").unwrap(),
        "fn a() {\n  f(|y| { # d\n  })\n}\n"
    );
    assert_eq!(
        format("fn a() {\n  x match { # e\n  }\n}\n").unwrap(),
        "fn a() {\n  x match { # e\n  }\n}\n"
    );
    assert_eq!(
        format("fn a() {\n  x escape { # e\n  }\n}\n").unwrap(),
        "fn a() {\n  x escape { # e\n  }\n}\n"
    );
}

/// An escape arm `_` (`[error.escape]`) is formatted as any other arm, on one line or in a block.
#[test]
fn escape_の_wildcard_arm() {
    let src = "fn or_else[A](f: fn() -> A, default: A) -> A {\n  f() escape { _ -> default }\n}\n\nfn a() {\n  config = load_config(path) escape {\n    NotFound(_) -> default_config()\n    _           -> panic(\"the bundled config must load\")\n  }\n}\n";
    assert_eq!(format(src).unwrap(), src);
}

/// A signature that does not fit moves its `fails` and `uses` clauses to their own lines first
/// (rule 4 of spec 2.5); the parameter list breaks only if the first line is still too long.
#[test]
fn 長い宣言は_fails_と_uses_から折る() {
    let src = "fn longsig(aaaa: Int, bbbb: String) -> Result[Int, String] fails NotFound | DbError uses { Io, Clock, Random, Http } {\n  x\n}\n";
    let expected = "fn longsig(aaaa: Int, bbbb: String) -> Result[Int, String]\n  fails NotFound | DbError\n  uses { Io, Clock, Random, Http } {\n  x\n}\n";
    assert_eq!(format(src).unwrap(), expected);
    // Narrower than the first line: the parameter list breaks too.
    let narrow = format_with_width(src, 40).unwrap();
    assert_eq!(
        narrow,
        "fn longsig(\n  aaaa: Int,\n  bbbb: String,\n) -> Result[Int, String]\n  fails NotFound | DbError\n  uses { Io, Clock, Random, Http } {\n  x\n}\n"
    );
    // A parameter list that breaks keeps the clauses on its last line while they fit.
    let params = "fn f(aaaaaaaa: Int, bbbbbbbb: Int, cccccccc: Int) -> Int fails Boom {\n  a\n}\n";
    assert_eq!(
        format_with_width(params, 40).unwrap(),
        "fn f(\n  aaaaaaaa: Int,\n  bbbbbbbb: Int,\n  cccccccc: Int,\n) -> Int fails Boom {\n  a\n}\n"
    );
    // One that fits stays on one line; a clause the source put on its own line stays there.
    assert_eq!(
        format("fn f() -> Int fails Boom uses { Log } { 1 }\n").unwrap(),
        "fn f() -> Int fails Boom uses { Log } { 1 }\n"
    );
    assert_eq!(
        format("fn f() -> Int\n  uses { Log } { 1 }\n").unwrap(),
        "fn f() -> Int\n  uses { Log } { 1 }\n"
    );
}

/// `[fmt.signature]`: a uses clause always has braces, and the old clause (E0147) becomes one.
#[test]
fn uses_節は波括弧で書き前の形の_use_は書き換える() {
    let cases = [
        // Braces around a bare name: an effect and a set type parameter alike.
        (
            "fn f() -> Int uses Db { 1 }\n",
            "fn f() -> Int uses { Db } { 1 }\n",
        ),
        (
            "fn f[R](g: fn() -> () uses R) uses R { g() }\n",
            "fn f[R](g: fn() -> () uses { R }) uses { R } { g() }\n",
        ),
        (
            "fn f() -> Int uses State[Int] { 1 }\n",
            "fn f() -> Int uses { State[Int] } { 1 }\n",
        ),
        // The old clause, with and without braces, in a signature and in a function type.
        (
            "fn f() -> Int use Db { 1 }\n",
            "fn f() -> Int uses { Db } { 1 }\n",
        ),
        (
            "fn f() -> Int fails E use { Db, Log } { 1 }\n",
            "fn f() -> Int fails E uses { Db, Log } { 1 }\n",
        ),
        (
            "fn f(g: fn() -> () fails {} use {}) { g() }\n",
            "fn f(g: fn() -> () fails {} uses {}) { g() }\n",
        ),
        (
            "trait Go {\n  fn go(self) -> () use Db\n}\n",
            "trait Go {\n  fn go(self) -> () uses { Db }\n}\n",
        ),
        // The old clause on its own line, which rule 4 no longer continues, stays there.
        (
            "fn f() -> Int\n  use Db {\n  1\n}\n",
            "fn f() -> Int\n  uses { Db } {\n  1\n}\n",
        ),
        (
            "@external(js, \"./x.mjs\", \"now\")\nfn now() -> Int\n  use {}\nfn g() { 1 }\n",
            "@external(js, \"./x.mjs\", \"now\")\nfn now() -> Int\n  uses {}\nfn g() { 1 }\n",
        ),
        // Already canonical.
        (
            "fn f() -> Int\n  fails Boom\n  uses { Db } {\n  1\n}\n",
            "fn f() -> Int\n  fails Boom\n  uses { Db } {\n  1\n}\n",
        ),
        // `use` as an expression and set type arguments of named types are not touched.
        (
            "fn f(h: Handler[E, Db]) {\n  log = use Log\n  (use Db).get()\n}\n",
            "fn f(h: Handler[E, Db]) {\n  log = use Log\n  (use Db).get()\n}\n",
        ),
        // `uses` as a name.
        (
            "fn f(uses: Int) -> Int {\n  uses + 1\n}\n",
            "fn f(uses: Int) -> Int {\n  uses + 1\n}\n",
        ),
    ];
    for (src, expected) in cases {
        let out = format(src).unwrap_or_else(|e| panic!("{src}: {e:?}"));
        assert_eq!(out, expected, "{src}");
        check("uses", src, 100);
    }
}

/// E0137 (a uses clause on an effect operation) is a syntax error for the formatter, also when
/// the clause is written in the old form.
#[test]
fn effect_operation_の_uses_節は整形しない() {
    for src in [
        "effect Store {\n  fn get() -> Int uses { Db }\n}\n",
        "effect Store {\n  fn get() -> Int use Db\n}\n",
    ] {
        let Err(FormatError::Syntax(diagnostics)) = format(src) else {
            panic!("{src} was formatted");
        };
        let codes: Vec<_> = diagnostics.iter().map(|d| d.code.as_str()).collect();
        assert_eq!(codes, ["E0137"], "{src}");
    }
}

#[test]
fn 補間の中は幅で折らない() {
    let src = "fn a() {\n  s = \"#{ f(|x| {\n    x\n  }) }\"\n}\n";
    let expected = "fn a() {\n  s = \"#{f(|x| {\n    x\n  })}\"\n}\n";
    for width in [100, 10] {
        assert_eq!(
            format_with_width(src, width).unwrap(),
            expected,
            "width {width}"
        );
    }
}

/// Changes indentation and the amount of whitespace between tokens, but not where newlines are.
fn reshuffle(src: &str, seeds: &[u8]) -> String {
    let tokens = emela_syntax::lex(src).tokens;
    let mut out = String::new();
    let mut seed = seeds.iter().cycle();
    let mut line_start = true;
    for t in tokens {
        let text = &src[t.range];
        match t.kind {
            emela_syntax::SyntaxKind::WHITESPACE => {
                let n = *seed.next().unwrap_or(&0) as usize % 4;
                // Indentation may be empty, but tokens need at least one space between them.
                let n = if line_start { n } else { n + 1 };
                out.push_str(&" ".repeat(n));
            }
            _ => out.push_str(text),
        }
        line_start = matches!(
            t.kind,
            emela_syntax::SyntaxKind::NEWLINE | emela_syntax::SyntaxKind::NEWLINE_CONT
        );
    }
    out
}

/// Adds or removes newlines between tokens right inside parentheses and brackets. Newlines
/// there are continuations (spec 2.5), so the reading does not change.
///
/// Newlines before a pipe (the hint for keeping the source's breaks), right after a comment,
/// and before a line comment are left alone.
fn rebreak(src: &str, seeds: &[u8]) -> String {
    use emela_syntax::SyntaxKind::*;
    let tokens = emela_syntax::lex(src).tokens;
    let mut out = String::new();
    let mut seed = seeds.iter().cycle();
    let mut stack = Vec::new();
    let mut prev = None;
    for (i, t) in tokens.iter().enumerate() {
        let text = &src[t.range];
        let in_parens = matches!(stack.last(), Some(L_PAREN | L_BRACK));
        let next = tokens[i + 1..]
            .iter()
            .map(|t| t.kind)
            .find(|k| !matches!(k, WHITESPACE | NEWLINE | NEWLINE_CONT));
        let choice = *seed.next().unwrap() % 4;
        match t.kind {
            NEWLINE_CONT
                if in_parens
                    && choice == 0
                    && !matches!(prev, Some(COMMENT | DOC_COMMENT))
                    && !matches!(next, Some(PIPE_GT | COMMENT | DOC_COMMENT) | None) =>
            {
                out.push(' ');
            }
            k if in_parens
                && choice == 1
                && !k.is_trivia()
                && k != NEWLINE
                && !matches!(
                    k,
                    PIPE_GT | STRING_QUOTE | TRIPLE_QUOTE | STRING_TEXT | INTERP_START
                ) =>
            {
                out.push('\n');
                out.push_str(text);
            }
            _ => out.push_str(text),
        }
        match t.kind {
            L_PAREN | L_BRACK | L_BRACE | INTERP_START => stack.push(t.kind),
            R_PAREN | R_BRACK | R_BRACE | INTERP_END => {
                stack.pop();
            }
            STRING_QUOTE if stack.last() == Some(&STRING_QUOTE) => {
                stack.pop();
            }
            STRING_QUOTE => stack.push(STRING_QUOTE),
            _ => {}
        }
        if !matches!(t.kind, WHITESPACE) {
            prev = Some(t.kind);
        }
    }
    out
}

/// Inserts a trailing comment before a newline and a line comment right after one. Comments are
/// skipped when deciding continuation, so the reading does not change.
fn add_comments(src: &str, seeds: &[u8]) -> String {
    use emela_syntax::SyntaxKind::*;
    let tokens = emela_syntax::lex(src).tokens;
    let mut out = String::new();
    let mut seed = seeds.iter().cycle();
    let mut prev = None;
    for (i, t) in tokens.iter().enumerate() {
        let text = &src[t.range];
        if matches!(t.kind, NEWLINE | NEWLINE_CONT) {
            let choice = *seed.next().unwrap() % 4;
            if choice == 0 && !matches!(prev, Some(COMMENT | DOC_COMMENT)) {
                out.push_str(&format!(" # t{i}"));
            }
            out.push_str(text);
            // A comment-only line before a `derive` on the next line makes the parser fail
            // (an emela-syntax issue; per spec 2.5 comment-only lines should be skipped).
            let next_is_derive = tokens[i + 1..]
                .iter()
                .find(|t| !t.kind.is_trivia() && t.kind != NEWLINE)
                .is_some_and(|t| &src[t.range] == "derive");
            if choice == 1 && !next_is_derive {
                out.push_str(&format!("# o{i}\n"));
            }
        } else {
            out.push_str(text);
        }
        if t.kind != WHITESPACE {
            prev = Some(t.kind);
        }
    }
    out
}

proptest! {
    // Each case formats every input, so keep the number of cases small.
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn 空白の量によらず同じ結果(seeds in proptest::collection::vec(any::<u8>(), 1..32)) {
        for (name, src) in inputs() {
            let shuffled = reshuffle(&src, &seeds);
            let expected = format(&src).unwrap();
            let got = format(&shuffled).unwrap_or_else(|e| panic!("{name}: {e}\n{shuffled}"));
            prop_assert_eq!(got, expected, "{}", name);
        }
    }

    /// The output is not compared with the original input, so `decls.emel` is included too
    /// (comments are also added on the line before `derive`).
    #[test]
    fn コメントを足しても壊れない(seeds in proptest::collection::vec(any::<u8>(), 1..64)) {
        for (name, src) in inputs() {
            let commented = add_comments(&src, &seeds);
            for width in [100, 30] {
                check(&format!("{name}\n{commented}"), &commented, width);
            }
        }
    }

    #[test]
    fn 括弧の中の改行によらず同じ結果(seeds in proptest::collection::vec(any::<u8>(), 1..64)) {
        for (name, src) in inputs() {
            let rebroken = rebreak(&src, &seeds);
            let expected = format(&src).unwrap();
            let got = format(&rebroken).unwrap_or_else(|e| panic!("{name}: {e}\n{rebroken}"));
            prop_assert_eq!(got, expected, "{}\n{}", name, rebroken);
        }
    }
}
