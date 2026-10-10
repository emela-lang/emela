//! 構文木を整形して出力する．
//!
//! 入力は `emela_syntax` の無損失構文木．コメントと空行は木に残っているので，
//! 整形はトークン列を作り直さずに木をたどって行う．折り返しは `pretty`（Wadler 式）に任せる．
//!
//! 構文エラーを含むファイルは整形しない．ERROR ノードの中身は文法どおりに並んでいないので，
//! 整形すると壊しうるため．

mod comments;
mod printer;

use std::fmt;

use emela_syntax::{Diagnostic, SyntaxKind, SyntaxNode, SyntaxToken};

/// 既定の幅．
pub const DEFAULT_WIDTH: usize = 100;

/// 整形できなかった理由．
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// 入力に構文エラーがある．診断は `emela_syntax::parse` のもの．
    Syntax(Vec<Diagnostic>),
    /// 整形の結果が入力と同じトークン列にならなかった，またはコメントが落ちた．
    /// formatter の不具合なので，入力を書き換えずに止める．
    Internal(String),
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormatError::Syntax(diagnostics) => {
                let n = diagnostics.len();
                let plural = if n == 1 { "" } else { "s" };
                write!(
                    f,
                    "cannot format a file with syntax errors ({n} error{plural})"
                )
            }
            FormatError::Internal(message) => write!(f, "internal formatter error: {message}"),
        }
    }
}

impl std::error::Error for FormatError {}

/// 既定の幅（100）で整形する．
pub fn format(src: &str) -> Result<String, FormatError> {
    format_with_width(src, DEFAULT_WIDTH)
}

/// 幅を指定して整形する．幅は1行の表示幅の目安で，収まらない字句（長い文字列など）は超える．
pub fn format_with_width(src: &str, width: usize) -> Result<String, FormatError> {
    let parse = emela_syntax::parse(src);
    if !parse.diagnostics().is_empty() {
        return Err(FormatError::Syntax(parse.diagnostics().to_vec()));
    }
    let root = parse.syntax();
    let mut printer = printer::Printer::new(&root, width);
    let doc = printer.root(&root);
    if printer.remaining_comments() != 0 {
        return Err(FormatError::Internal(format!(
            "{} comment(s) were not printed",
            printer.remaining_comments()
        )));
    }
    let out = finish(&printer::render(&doc, width));
    verify(&root, &out)?;
    Ok(out)
}

/// 行末の空白を落とし，最後を改行1つで終える．
///
/// 複数行の文字列 `"""` の中の改行の手前の空白は値の一部なので残す．
fn finish(rendered: &str) -> String {
    // 文字列の中にある改行の位置．
    let mut in_string = std::collections::HashSet::new();
    for token in emela_syntax::lex(rendered).tokens {
        if matches!(
            token.kind,
            SyntaxKind::STRING_TEXT | SyntaxKind::TRIPLE_QUOTE
        ) {
            let start = usize::from(token.range.start());
            for (i, _) in rendered[token.range].match_indices('\n') {
                in_string.insert(start + i);
            }
        }
    }
    let mut out = String::with_capacity(rendered.len());
    let mut offset = 0;
    for line in rendered.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let newline_at = offset + body.len();
        offset += line.len();
        let body = body.strip_suffix('\r').unwrap_or(body);
        if in_string.contains(&newline_at) {
            out.push_str(body);
        } else {
            out.push_str(body.trim_end());
        }
        out.push('\n');
    }
    let trimmed = out.trim_end_matches('\n').len();
    out.truncate(trimmed);
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// 整形の前後で意味が変わっていないことを確かめる．結果は構文エラーなく読め，
/// トークン列（`normalized_tokens`）とコメントの並びが入力と一致しなければならない．
fn verify(before: &SyntaxNode, out: &str) -> Result<(), FormatError> {
    let parse = emela_syntax::parse(out);
    if let Some(d) = parse.diagnostics().first() {
        return Err(FormatError::Internal(format!(
            "the output has a syntax error: {} at {:?}",
            d.message, d.range
        )));
    }
    let after = parse.syntax();
    if normalized_tokens(before) != normalized_tokens(&after) {
        return Err(FormatError::Internal(
            "the output changed the tokens".into(),
        ));
    }
    if comment_texts(before) != comment_texts(&after) {
        return Err(FormatError::Internal(
            "the output changed the comments".into(),
        ));
    }
    Ok(())
}

/// 意味に関わるトークンの列．整形で変わってよいものを除く．
///
/// - トリビア（空白，継続の改行，コメント）
/// - 並びの末尾のカンマ（閉じ括弧の直前のカンマ）
/// - 文を区切る改行のうち，ブロックの `{` の直後と `}` の直前，カンマの前後，入力の
///   始まりと終わりのもの．続いた改行は1つにまとめる
/// - 区切りとして働かない改行（注釈の後ろ，`derive` の前など）
/// - 残った改行は `,` に置き換える（enum のバリアントなどはどちらでも区切れる）
#[doc(hidden)]
pub fn normalized_tokens(root: &SyntaxNode) -> Vec<(SyntaxKind, String)> {
    use SyntaxKind::*;
    let tokens: Vec<_> = root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia())
        .filter(|t| t.kind() != NEWLINE || separates(t))
        // 複数行の文字列の中の `\r\n` は `\n` と同じ改行として比べる．
        .map(|t| (t.kind(), t.text().replace("\r\n", "\n")))
        .collect();
    let mut out: Vec<(SyntaxKind, String)> = Vec::new();
    for (i, (kind, text)) in tokens.iter().enumerate() {
        let next = tokens[i + 1..]
            .iter()
            .map(|(k, _)| *k)
            .find(|k| *k != NEWLINE);
        match kind {
            NEWLINE => {
                let prev = out.last().map(|(k, _)| *k);
                let drop = matches!(prev, None | Some(NEWLINE | L_BRACE | COMMA))
                    || matches!(next, None | Some(R_BRACE | COMMA));
                // 残った改行は区切りとして `,` と同じに扱う．enum のバリアントなどは
                // カンマでも改行でも区切れ，整形で入れ替わるため．`\r\n` と `\n` も区別しない．
                if !drop {
                    out.push((COMMA, ",".to_owned()));
                }
            }
            COMMA if matches!(next, Some(R_PAREN | R_BRACK | R_BRACE)) => {}
            _ => out.push((*kind, text.clone())),
        }
    }
    out
}

/// 改行が区切りとして働くか．ファイル，ブロック，腕の並び，宣言の本体の改行だけが
/// 区切りで，注釈の後ろや `derive` の前，効果の集合の中の改行はパーサが読み飛ばす．
fn separates(newline: &SyntaxToken) -> bool {
    use SyntaxKind::*;
    newline.parent().is_some_and(|p| {
        matches!(
            p.kind(),
            ROOT | BLOCK_EXPR | MATCH_ARM_LIST | ITEM_LIST | VARIANT_LIST
        )
    })
}

/// コメントの本文の並び（行末の空白を除く）．
#[doc(hidden)]
pub fn comment_texts(root: &SyntaxNode) -> Vec<String> {
    root.descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| matches!(t.kind(), SyntaxKind::COMMENT | SyntaxKind::DOC_COMMENT))
        .map(|t| t.text().trim_end().to_owned())
        .collect()
}
