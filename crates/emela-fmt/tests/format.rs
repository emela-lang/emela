//! formatter のテスト．`test_data/*.emel` を整形した結果をスナップショットで比べ，
//! どの入力でも次が成り立つことを確かめる．
//!
//! - 冪等: `format(format(x)) == format(x)`
//! - 意味を変えない: trivia，末尾のカンマ，ブロックの端の改行を除いたトークン列が一致する
//! - コメントが落ちない: コメントの本文の並びが一致する
//!
//! 幅を変えると折り返し方が変わるので，性質の検査はいくつかの幅で行う．

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

/// 整形の結果について，冪等・トークン列・コメントを確かめる．
fn check(name: &str, src: &str, width: usize) -> String {
    let out = format_with_width(src, width)
        .unwrap_or_else(|e| panic!("{name}（幅 {width}）を整形できない: {e}"));
    let again = format_with_width(&out, width).unwrap();
    assert_eq!(again, out, "{name}（幅 {width}）が冪等でない");
    let before = parse(src).syntax();
    let after = parse(&out).syntax();
    assert_eq!(
        normalized_tokens(&before),
        normalized_tokens(&after),
        "{name}（幅 {width}）のトークン列が変わった"
    );
    assert_eq!(
        comment_texts(&before),
        comment_texts(&after),
        "{name}（幅 {width}）のコメントが変わった"
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

/// 狭い幅での折り返し．
#[test]
fn narrow_snapshots() {
    for name in ["long.emel", "comments.emel", "decls.emel"] {
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
    // 文を区切る改行が比べられる位置にあっても，`\r\n` と `\n` は同じ改行として扱う．
    assert_eq!(
        format("fn a() {\r\n  x = 1\r\n  y = 2\r\n}\r\n\r\nfn b() {}\r\n").unwrap(),
        "fn a() {\n  x = 1\n  y = 2\n}\n\nfn b() {}\n"
    );
}

#[test]
fn 中身のないブロックの行末のコメントの後に空行を入れない() {
    assert_eq!(format("fn a() { # c\n}\n").unwrap(), "fn a() { # c\n}\n");
    assert_eq!(
        format("fn a() {\n  f(fn(y) { # d\n  })\n}\n").unwrap(),
        "fn a() {\n  f(fn(y) { # d\n  })\n}\n"
    );
    assert_eq!(
        format("fn a() {\n  match x { # e\n  }\n}\n").unwrap(),
        "fn a() {\n  match x { # e\n  }\n}\n"
    );
    assert_eq!(
        format("fn a() {\n  x escape { # e\n  }\n}\n").unwrap(),
        "fn a() {\n  x escape { # e\n  }\n}\n"
    );
}

#[test]
fn 長い宣言は引数の並びから折る() {
    let src = "fn longsig(aaaa: Int, bbbb: String) -> Result[Int, String] fails NotFound | DbError use { Io, Clock, Random, Http } {\n  x\n}\n";
    let expected = "fn longsig(\n  aaaa: Int,\n  bbbb: String,\n) -> Result[Int, String] fails NotFound | DbError use { Io, Clock, Random, Http } {\n  x\n}\n";
    assert_eq!(format(src).unwrap(), expected);
    let narrow = format_with_width(src, 40).unwrap();
    assert!(
        narrow.contains(") -> Result[Int, String] fails"),
        "{narrow}"
    );
}

#[test]
fn 補間の中は幅で折らない() {
    let src = "fn a() {\n  s = \"#{ f(fn(x) {\n    x\n  }) }\"\n}\n";
    let expected = "fn a() {\n  s = \"#{f(fn(x) {\n    x\n  })}\"\n}\n";
    for width in [100, 10] {
        assert_eq!(
            format_with_width(src, width).unwrap(),
            expected,
            "幅 {width}"
        );
    }
}

/// 字下げと，トークンの間の空白の量を変える．改行の位置は変えない．
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
                // 行頭の字下げは0個でもよいが，トークンの間は1つ以上要る．
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

/// 丸括弧と角括弧のすぐ内側で，トークンの間の改行を足したり除いたりする．
/// そこでは改行が継続になる（2.5）ので，読み方は変わらない．
///
/// パイプの前の改行（元の折り方を保つ手がかり），コメントの直後の改行，行だけの
/// コメントの前の改行は変えない．
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
                && !matches!(k, PIPE_GT | STRING_QUOTE | STRING_TEXT | INTERP_START) =>
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

/// 改行の手前に行末のコメントを，改行の直後に行だけのコメントを差し込む．
/// コメントは継続の判定で読み飛ばされるので，読み方は変わらない．
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
            // 次の行の `derive` の前にコメントだけの行を挟むと，パーサが読めない
            // （emela-syntax の課題．2.5 ではコメントだけの行は読み飛ばすはず）．
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
    // 1件ごとに全部の入力を整形するので，件数は少なめにする．
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// 空白の量だけが違う入力は，同じ結果になる．
    #[test]
    fn 空白の量によらず同じ結果(seeds in proptest::collection::vec(any::<u8>(), 1..32)) {
        for (name, src) in inputs() {
            let shuffled = reshuffle(&src, &seeds);
            let expected = format(&src).unwrap();
            let got = format(&shuffled).unwrap_or_else(|e| panic!("{name}: {e}\n{shuffled}"));
            prop_assert_eq!(got, expected, "{}", name);
        }
    }

    /// どこにコメントを足しても，整形は冪等で，トークン列とコメントを変えない．
    #[test]
    fn コメントを足しても壊れない(seeds in proptest::collection::vec(any::<u8>(), 1..64)) {
        for (name, src) in inputs() {
            let commented = add_comments(&src, &seeds);
            for width in [100, 30] {
                check(&format!("{name}\n{commented}"), &commented, width);
            }
        }
    }

    /// 括弧の中の改行の位置だけが違う入力は，同じ結果になる．
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
