//! Context-aware completion (spec 0033). The cursor's context is classified by
//! scanning the token stream before it — the error-tolerant lexer always
//! produces one — and candidates come from the scope snapshot the last check
//! extracted, so completion keeps working while the file has errors.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::ast::{Block, BlockItem, Expr, Function, Type};
use crate::driver;
use crate::lexer::{Token, TokenKind, lex};
use crate::lsp::analysis::{EnumSym, Snapshot, render_fn_sig, render_type};
use crate::lsp::documents::Document;
use crate::lsp::position::position_to_offset;
use crate::lsp::protocol::{CompletionItem, Position, completion_kind};
use crate::parser::parse_program;

/// Emela's reserved words (the lexer's keyword table).
const KEYWORDS: &[&str] = &[
    "fn",
    "extern",
    "intrinsic",
    "trait",
    "impl",
    "for",
    "import",
    "let",
    "module",
    "pub",
    "uses",
    "enum",
    "match",
    "if",
    "else",
    "throws",
    "throw",
    "try",
    "catch",
    "panic",
    "true",
    "false",
];

/// The built-in type names `parse_type` recognizes.
const TYPE_NAMES: &[&str] = &[
    "Unit", "Bool", "Int", "Float", "String", "Char", "Array", "Never", "Record", "Function",
    "Self",
];

pub(crate) fn complete(
    doc: &Document,
    position: &Position,
    snapshot: &Snapshot,
    package_paths: &[PathBuf],
) -> Vec<CompletionItem> {
    let offset = position_to_offset(&doc.text, position);

    // 1. An `import` line completes package/module/item paths, not code.
    let line_start = doc.text[..offset].rfind('\n').map_or(0, |index| index + 1);
    let line = &doc.text[line_start..offset];
    if let Some(rest) = line.trim_start().strip_prefix("import")
        && rest.starts_with([' ', '\t']) | rest.is_empty()
    {
        return complete_import(doc, rest.trim_start(), package_paths);
    }

    let (tokens, _) = lex("<completion>", &doc.text);
    let mut before: Vec<Token> = tokens
        .into_iter()
        .filter(|token| token.span.end <= offset && !matches!(token.kind, TokenKind::Eof))
        .collect();
    // The word being typed is the client-side filter prefix, not context.
    if before
        .last()
        .is_some_and(|token| token.span.end == offset && is_word(&token.kind))
    {
        before.pop();
    }

    // 2. `Enum::` completes that enum's variants (specs 0005/0017/0018 R7).
    if before
        .last()
        .is_some_and(|t| t.kind == TokenKind::ColonColon)
        && let Some(Token {
            kind: TokenKind::Ident(name),
            ..
        }) = before.get(before.len().wrapping_sub(2))
    {
        return complete_type_path(name, snapshot);
    }

    // 3–5. Contexts keyed to the innermost unclosed `{`.
    if let Some(open) = innermost_unclosed_brace(&before) {
        let at_pattern = matches!(
            before.last().map(|t| &t.kind),
            Some(TokenKind::LBrace | TokenKind::Newline | TokenKind::Comma)
        );
        match before[..open].last().map(|t| &t.kind) {
            // 3. `uses { … ` — effect names (specs 0009/0022).
            Some(TokenKind::Uses) => {
                return snapshot
                    .effects
                    .iter()
                    .map(|effect| {
                        CompletionItem::new(effect, completion_kind::EVENT).detail("effect")
                    })
                    .collect();
            }
            // 5. `catch { … ` — variants of the error enums in scope (spec 0011).
            Some(TokenKind::Catch) if at_pattern => {
                return complete_catch_arms(snapshot);
            }
            _ => {
                // 4. `match scrutinee { … ` — the scrutinee's variants (spec 0005).
                if at_pattern && let Some(scrutinee) = enclosing_match_scrutinee(&before, open) {
                    return complete_match_arms(&scrutinee, offset, snapshot);
                }
            }
        }
    }

    // 6. Default: keywords, type names, and everything in scope.
    default_items(offset, snapshot)
}

fn is_word(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Ident(_)
            | TokenKind::Fn
            | TokenKind::Extern
            | TokenKind::Intrinsic
            | TokenKind::Trait
            | TokenKind::Impl
            | TokenKind::For
            | TokenKind::Import
            | TokenKind::Let
            | TokenKind::Module
            | TokenKind::Pub
            | TokenKind::Uses
            | TokenKind::Enum
            | TokenKind::Match
            | TokenKind::If
            | TokenKind::Else
            | TokenKind::Throws
            | TokenKind::Throw
            | TokenKind::Try
            | TokenKind::Catch
            | TokenKind::Panic
            | TokenKind::True
            | TokenKind::False
    )
}

/// The index of the innermost `{` in `before` that the cursor is still inside.
fn innermost_unclosed_brace(before: &[Token]) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in before.iter().enumerate().rev() {
        match token.kind {
            TokenKind::RBrace => depth += 1,
            TokenKind::LBrace => {
                if depth == 0 {
                    return Some(index);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

/// If the unclosed `{` at `open` is a `match` block, the scrutinee tokens
/// between `match` and the `{`. A scrutinee stays on one line, so the backward
/// scan stops at a newline or another block boundary.
fn enclosing_match_scrutinee(before: &[Token], open: usize) -> Option<Vec<TokenKind>> {
    let mut index = open;
    while index > 0 {
        index -= 1;
        match &before[index].kind {
            TokenKind::Match => {
                return Some(
                    before[index + 1..open]
                        .iter()
                        .map(|token| token.kind.clone())
                        .collect(),
                );
            }
            // Any other block/line boundary means the `{` belongs to something
            // else (`if`, `try`, a function body, …).
            TokenKind::Newline
            | TokenKind::LBrace
            | TokenKind::RBrace
            | TokenKind::If
            | TokenKind::Else
            | TokenKind::Try
            | TokenKind::Catch
            | TokenKind::Fn => return None,
            _ => {}
        }
    }
    None
}

/// Enum variants offered right after `Name::`. The former `Char::from_code` /
/// `String::from_char` conversions are now bare intrinsics (spec 0021), so `::`
/// completes enum variants only.
fn complete_type_path(name: &str, snapshot: &Snapshot) -> Vec<CompletionItem> {
    let Some(sym) = snapshot.enums.iter().find(|sym| sym.name == name) else {
        return Vec::new();
    };
    sym.variants
        .iter()
        .map(|variant| {
            let item = CompletionItem::new(&variant.name, completion_kind::ENUM_MEMBER)
                .detail(format!("variant of {}", sym.name));
            if variant.arity == 0 {
                item
            } else {
                item.snippet(format!(
                    "{}({})",
                    variant.name,
                    placeholders(variant.arity, "value")
                ))
            }
        })
        .collect()
}

/// `${1:v1}, ${2:v2}, …` snippet tab stops for an n-field payload.
fn placeholders(arity: usize, stem: &str) -> String {
    (1..=arity)
        .map(|index| format!("${{{index}:{stem}{index}}}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Arm-position items for `catch { … }` (spec 0011): enums named in a `throws`
/// clause in scope come first, every other enum after.
fn complete_catch_arms(snapshot: &Snapshot) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    for sym in &snapshot.enums {
        let group = if snapshot.throws_enums.contains(&sym.name) {
            '0'
        } else {
            '1'
        };
        items.extend(pattern_items(sym, group));
    }
    items.push(wildcard_item());
    items
}

/// Arm-position items for `match scrutinee { … }` (spec 0005). When the
/// scrutinee's enum can be pinned down from the enclosing function's
/// parameters or annotated `let`s, its variants come first.
fn complete_match_arms(
    scrutinee: &[TokenKind],
    offset: usize,
    snapshot: &Snapshot,
) -> Vec<CompletionItem> {
    let scrutinee_enum = match scrutinee {
        [TokenKind::Ident(name)] => scrutinee_type(name, offset, snapshot),
        _ => None,
    };
    // `Option` is an ordinary Core-Prelude enum (spec 0042), so its `Some`/`None`
    // arms come from `snapshot.enums` through the general path below.
    let scrutinee_enum = match &scrutinee_enum {
        Some(Type::Enum(name, _)) => Some(name.clone()),
        _ => None,
    };
    let mut items = Vec::new();
    for sym in &snapshot.enums {
        match &scrutinee_enum {
            Some(name) if name == &sym.name => items.extend(pattern_items(sym, '0')),
            Some(_) => {}
            None => items.extend(pattern_items(sym, '1')),
        }
    }
    items.push(wildcard_item());
    items
}

/// Pattern completions for one enum: the bare variant name, with binding
/// placeholders for a payload (`Variant(${1:v1})`).
fn pattern_items(sym: &EnumSym, group: char) -> Vec<CompletionItem> {
    sym.variants
        .iter()
        .map(|variant| {
            let item = CompletionItem::new(
                format!("{}::{}", sym.name, variant.name),
                completion_kind::ENUM_MEMBER,
            )
            .detail(format!("variant of {}", sym.name))
            .sort_group(group);
            if variant.arity == 0 {
                item
            } else {
                item.snippet(format!(
                    "{}::{}({})",
                    sym.name,
                    variant.name,
                    placeholders(variant.arity, "v")
                ))
            }
        })
        .collect()
}

fn wildcard_item() -> CompletionItem {
    CompletionItem::new("_", completion_kind::KEYWORD)
        .detail("wildcard pattern")
        .sort_group('2')
}

/// The declared type of `name` at `offset`: an enclosing function's parameter
/// or the nearest annotated `let` above the cursor. Best-effort — inference
/// beyond annotations is out of scope for completion (spec 0033). Code
/// actions share it as the fallback when the type index has no entry.
pub(crate) fn scrutinee_type(name: &str, offset: usize, snapshot: &Snapshot) -> Option<Type> {
    let function = enclosing_function(offset, snapshot)?;
    let mut found = None;
    for param in &function.params {
        if param.name == name {
            found = Some(param.ty.clone());
        }
    }
    let mut lets = Vec::new();
    collect_lets(&function.body, offset, &mut lets);
    for (let_name, ty) in lets {
        if let_name == name && ty.is_some() {
            found = ty;
        }
    }
    found
}

/// The innermost entry-file function whose body contains `offset`.
fn enclosing_function(offset: usize, snapshot: &Snapshot) -> Option<&Function> {
    snapshot
        .entry_functions
        .iter()
        .filter(|function| function.body.span.start <= offset && offset <= function.body.span.end)
        .min_by_key(|function| function.body.span.end - function.body.span.start)
}

/// Every `let` binding above `offset` in `block`, innermost last, with its
/// annotated type when present.
fn collect_lets(block: &Block, offset: usize, out: &mut Vec<(String, Option<Type>)>) {
    for item in &block.items {
        match item {
            BlockItem::Let {
                name,
                name_span,
                ty,
                value,
            } => {
                if name_span.start < offset {
                    out.push((name.clone(), ty.clone()));
                }
                collect_lets_expr(value, offset, out);
            }
            BlockItem::Expr(expr) => collect_lets_expr(expr, offset, out),
        }
    }
}

fn collect_lets_expr(expr: &Expr, offset: usize, out: &mut Vec<(String, Option<Type>)>) {
    let span = expr.span();
    if offset < span.start || span.end < offset {
        return;
    }
    match expr {
        Expr::Block(block) => collect_lets(block, offset, out),
        Expr::If { then, els, .. } => {
            collect_lets(then, offset, out);
            collect_lets(els, offset, out);
        }
        Expr::Match { arms, .. } | Expr::Try { arms, .. } => {
            for arm in arms {
                collect_lets_expr(&arm.body, offset, out);
            }
        }
        Expr::Fn { body, .. } => collect_lets(body, offset, out),
        _ => {}
    }
}

/// The default context (spec 0033): reserved words, built-in type names, and
/// the functions, enums, trait methods, and locals in scope.
fn default_items(offset: usize, snapshot: &Snapshot) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    for keyword in KEYWORDS {
        items.push(
            CompletionItem::new(*keyword, completion_kind::KEYWORD)
                .detail("keyword")
                .sort_group('3'),
        );
    }
    for name in TYPE_NAMES {
        items.push(
            CompletionItem::new(*name, completion_kind::CLASS)
                .detail("type")
                .sort_group('2'),
        );
    }
    let mut seen = BTreeSet::new();
    if let Some(function) = enclosing_function(offset, snapshot) {
        for param in &function.params {
            if seen.insert(param.name.clone()) {
                items.push(
                    CompletionItem::new(&param.name, completion_kind::VARIABLE)
                        .detail(render_type(&param.ty))
                        .sort_group('0'),
                );
            }
        }
        let mut lets = Vec::new();
        collect_lets(&function.body, offset, &mut lets);
        for (name, ty) in lets {
            if seen.insert(name.clone()) {
                let item = CompletionItem::new(&name, completion_kind::VARIABLE).sort_group('0');
                items.push(match ty {
                    Some(ty) => item.detail(render_type(&ty)),
                    None => item,
                });
            }
        }
    }
    for function in &snapshot.functions {
        items.push(
            CompletionItem::new(&function.name, completion_kind::FUNCTION)
                .detail(&function.detail)
                .sort_group('1'),
        );
    }
    for method in &snapshot.trait_methods {
        items.push(
            CompletionItem::new(&method.name, completion_kind::METHOD)
                .detail(&method.detail)
                .sort_group('1'),
        );
    }
    for sym in &snapshot.enums {
        items.push(
            CompletionItem::new(&sym.name, completion_kind::ENUM)
                .detail("enum")
                .sort_group('1'),
        );
    }
    items
}

/// Completion on an `import` line (spec 0018/0032): the dotted prefix decides
/// whether package roots, module files, or a module's public functions are
/// offered.
fn complete_import(doc: &Document, prefix: &str, package_paths: &[PathBuf]) -> Vec<CompletionItem> {
    let doc_dir = doc
        .path
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf);
    // Segments already completed with a `.`; the trailing partial word (if
    // any) is the client-side filter prefix.
    let complete: Vec<&str> = if prefix.is_empty() {
        Vec::new()
    } else if prefix.ends_with('.') {
        prefix.trim_end_matches('.').split('.').collect()
    } else {
        let mut segments: Vec<&str> = prefix.split('.').collect();
        segments.pop();
        segments
    };

    let packages = doc
        .path
        .as_deref()
        .and_then(|path| driver::load_import_roots(path, package_paths).ok())
        .unwrap_or_default();

    if complete.is_empty() {
        let mut items: Vec<CompletionItem> = packages
            .iter()
            .map(|package| {
                CompletionItem::new(package.name(), completion_kind::MODULE).detail("package")
            })
            .collect();
        if let Some(dir) = &doc_dir {
            let own = doc
                .path
                .as_deref()
                .and_then(Path::file_stem)
                .map(|stem| stem.to_string_lossy().into_owned());
            for stem in module_stems(dir) {
                if Some(&stem) != own.as_ref() {
                    items.push(CompletionItem::new(stem, completion_kind::MODULE).detail("module"));
                }
            }
        }
        return items;
    }

    // Resolve the completed segments against a package root or the document's
    // directory (relative import), mirroring `imports::resolve_module_file`.
    let (root, parts): (PathBuf, &[&str]) = match packages
        .iter()
        .find(|package| package.name() == complete[0])
    {
        Some(package) => (package.source_root().to_path_buf(), &complete[1..]),
        None => match &doc_dir {
            Some(dir) => (dir.clone(), &complete[..]),
            None => return Vec::new(),
        },
    };

    let mut items = Vec::new();
    // Deeper module levels: subdirectories and module files under the prefix.
    let dir = parts
        .iter()
        .fold(root.clone(), |path, part| path.join(part));
    for stem in module_stems(&dir) {
        items.push(CompletionItem::new(stem, completion_kind::MODULE).detail("module"));
    }
    // Item level: the prefix names a module file — offer its `pub fn`s.
    if !parts.is_empty() {
        let mut file = parts.iter().fold(root, |path, part| path.join(part));
        file.set_extension("emel");
        if let Ok(source) = fs::read_to_string(&file) {
            let (program, _) = parse_program(&file.display().to_string(), &source);
            for function in &program.functions {
                if function.is_public {
                    items.push(
                        CompletionItem::new(&function.name, completion_kind::FUNCTION).detail(
                            render_fn_sig(
                                &function.name,
                                &function
                                    .params
                                    .iter()
                                    .map(|param| param.ty.clone())
                                    .collect::<Vec<_>>(),
                                &function.ret,
                                &function.throws,
                                &function.effects,
                            ),
                        ),
                    );
                }
            }
        }
    }
    items
}

/// The importable names under `dir`: subdirectories and `.emel` file stems.
fn module_stems(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut stems = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(name) = path.file_name() {
                stems.push(name.to_string_lossy().into_owned());
            }
        } else if path.extension().is_some_and(|ext| ext == "emel")
            && let Some(stem) = path.file_stem()
        {
            stems.push(stem.to_string_lossy().into_owned());
        }
    }
    stems.sort();
    stems.dedup();
    stems
}
