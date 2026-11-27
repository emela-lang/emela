//! Diagnostic-linked edits. All locations refer to the current analysis snapshot.

use emela_resolve::ModuleId;
use emela_resolve::hir::{DefId, DefKind, DefModule, ExprKind, Item, RefTarget, Span};
use emela_syntax::ast::{self, AstNode};
use emela_syntax::{DiagnosticCode, SyntaxKind, SyntaxNode, rewrite};
use emela_types::{DiagnosticKind, EffectContext, Ty};
use line_index::{TextRange, TextSize};

use super::edit::{Edit, Fix, append, indent};
use super::{CodeAction, CodeActionKind, Names, clause_edit, visible};
use crate::code;
use crate::diagnostic::{Diagnostic, Location};
use crate::frontend::TypedProgram;
use crate::pipeline::Analysis;
use crate::source::FileId;

pub(super) fn quickfixes(
    program: &TypedProgram,
    analysis: &Analysis,
    module: ModuleId,
    file: FileId,
    range: TextRange,
) -> Vec<CodeAction> {
    let text = analysis.sources[file].text();
    let tree = emela_syntax::parse(text).tree();
    let mut actions = Vec::new();
    for diagnostic in &analysis.diagnostics {
        let Some(Location::Span(span)) = diagnostic.location else {
            continue;
        };
        if span.file != file || !touches(span.range, range) {
            continue;
        }
        let edits = match diagnostic.code {
            Some(code::NON_EXHAUSTIVE_MATCH) => {
                missing_arms(program, analysis, module, span.range, &tree, text)
            }
            Some(code::INCOMPLETE_IMPL) => {
                super::impls::insert_stubs(&program.hir, module, span.range, &tree, text)
            }
            Some(code::UNDECLARED_ERRORS | code::UNDECLARED_EFFECTS) => {
                extend_annotation(program, analysis, module, diagnostic, &tree)
            }
            Some(c) if c == DiagnosticCode::OldImplHead.as_str() => {
                old_impl_head(tree.syntax(), span.range)
            }
            Some(c) if c == DiagnosticCode::OldHandlerHead.as_str() => {
                old_handler_head(tree.syntax(), span.range)
            }
            Some(c) if c == DiagnosticCode::OldLambda.as_str() => {
                old_lambda(tree.syntax(), span.range)
            }
            Some(c) if c == DiagnosticCode::OldUseClause.as_str() => {
                old_uses_clause(tree.syntax(), span.range)
            }
            Some(code::DEPRECATED_USE) => deprecated_use(tree.syntax(), diagnostic, span.range),
            Some(code::UNUSED_TYPE_PARAM) => {
                unused_type_param(program, module, span.range, tree.syntax())
            }
            _ => None,
        };
        if let Some((title, edits)) = edits {
            actions.push(fix(title, diagnostic, edits));
        }
        if diagnostic.code == Some(code::UNDEFINED_NAME) {
            actions.extend(imports(
                program, analysis, module, diagnostic, span.range, &tree,
            ));
        }
        if matches!(
            diagnostic.code,
            Some(code::UNDEFINED_NAME | code::UNDEFINED_MEMBER)
        ) && let Some(name) = nearby_name(program, analysis, file, span.range)
        {
            actions.push(fix(
                format!("Replace with `{name}`"),
                diagnostic,
                vec![(span.range, name.to_string())],
            ));
        }
    }
    actions
}

fn fix(title: String, diagnostic: &Diagnostic, edits: Vec<Edit>) -> CodeAction {
    CodeAction {
        title,
        kind: CodeActionKind::QuickFix,
        diagnostics: vec![diagnostic.clone()],
        edits,
    }
}

/// `impl Show for User` -> `impl User: Show` (E1402, spec 10.2).
fn old_impl_head(root: &SyntaxNode, range: TextRange) -> Option<Fix> {
    let (range, head) = root
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::IMPL_DECL)
        .find_map(|n| rewrite::old_impl_head(&n).filter(|(r, _)| *r == range))?;
    Some((format!("Rewrite as `{head}`"), vec![(range, head)]))
}

/// `handler H implements E` -> `handler H: E` (E1403, spec 8.4).
fn old_handler_head(root: &SyntaxNode, range: TextRange) -> Option<Fix> {
    let (_, edit, head) = root
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::HANDLER_DECL)
        .find_map(|n| rewrite::old_handler_head(&n).filter(|(t, _, _)| t.text_range() == range))?;
    Some((format!("Rewrite as `{head}`"), vec![(edit, ":".to_owned())]))
}

/// `fn(x) { x + 1 }` -> `|x| x + 1` (E1405, `[fn.decl]`). The diagnostic covers `fn` and the
/// parameter list; the edit replaces the whole function.
fn old_lambda(root: &SyntaxNode, range: TextRange) -> Option<Fix> {
    let (edit, text, head) = root
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::LAMBDA_EXPR && n.text_range().start() == range.start())
        .find_map(|n| rewrite::old_lambda(&n))?;
    Some((format!("Rewrite as `{head} ...`"), vec![(edit, text)]))
}

/// `use Db` -> `uses { Db }` (E1404, `[lsp.code-action]`). The diagnostic points at `use`; the
/// whole clause is replaced with its canonical form.
fn old_uses_clause(root: &SyntaxNode, range: TextRange) -> Option<Fix> {
    let (clause, uses) = root
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::USES_CLAUSE)
        .find_map(|n| {
            rewrite::old_uses_clause(&n)
                .filter(|(t, _)| t.text_range() == range)
                .map(|(_, uses)| (n, uses))
        })?;
    Some((
        format!("Rewrite as `{uses}`"),
        vec![(clause.text_range(), uses)],
    ))
}

/// A type parameter that does not appear in the type of its fn (W3002, `[lsp.code-action]`) is
/// removed with its bound and the `,` next to it, and the brackets with it when it is the only
/// one. Not offered when it is named in the body of the fn: removing it would make that annotation
/// E2101.
fn unused_type_param(
    program: &TypedProgram,
    module: ModuleId,
    range: TextRange,
    root: &SyntaxNode,
) -> Option<Fix> {
    let hir = &program.hir;
    let span = Span { module, range };
    let (param, data) = hir
        .defs
        .iter()
        .find(|(_, d)| d.kind == DefKind::TypeParam && d.span == Some(span))?;
    let body = hir.defs.iter().find_map(|(def, _)| match hir.item(def) {
        Some(Item::Fn(item) | Item::TraitFn(item))
            if item.sig.type_params.iter().any(|tp| tp.def == param) =>
        {
            Some(item.body)
        }
        _ => None,
    })?;
    if let Some(body) = body {
        let body = hir.exprs[body].span;
        let named_in_body = hir.refs.iter().any(|r| {
            r.target == RefTarget::Def(param)
                && r.span.module == body.module
                && body.range.contains_range(r.span.range)
        });
        if named_in_body {
            return None;
        }
    }
    let node = root
        .descendants()
        .find(|n| n.kind() == SyntaxKind::TYPE_PARAM && n.text_range().start() == range.start())?;
    let list = node
        .parent()
        .filter(|n| n.kind() == SyntaxKind::TYPE_PARAM_LIST)?;
    let params: Vec<SyntaxNode> = list
        .children()
        .filter(|n| n.kind() == SyntaxKind::TYPE_PARAM)
        .collect();
    let at = params.iter().position(|n| *n == node)?;
    let removed = if params.len() == 1 {
        list.text_range()
    } else if let Some(next) = params.get(at + 1) {
        TextRange::new(node.text_range().start(), next.text_range().start())
    } else {
        TextRange::new(params[at - 1].text_range().end(), node.text_range().end())
    };
    Some((
        format!("Remove type parameter `{}`", data.name),
        vec![(removed, String::new())],
    ))
}

/// `use E` written as an expression (W4401, `[effect.use]`), rewritten to an effect alias when
/// the help names one: name resolution gives that help only where the alias means the same.
///
/// - `x = use E` -> `x <- E`.
/// - `(use E).op(...)` -> `a.op(...)`, with `a <- E` inserted before the statement of the
///   innermost block that contains it. An effect alias resolves at the call, so the call reaches
///   the same handler wherever the alias is declared in the function. If the name is taken in the
///   function, a number is added to it.
fn deprecated_use(root: &SyntaxNode, diagnostic: &Diagnostic, range: TextRange) -> Option<Fix> {
    let help = diagnostic
        .notes
        .iter()
        .find_map(|n| n.strip_prefix("help: "))?;
    let use_expr = root
        .descendants()
        .find(|n| n.kind() == SyntaxKind::USE_EXPR && n.text_range() == range)?;
    let effect = use_expr
        .text()
        .to_string()
        .trim_start_matches("use")
        .trim()
        .to_owned();
    if !help.contains(" before the statement") {
        // `write `x <- E``: the binding `x = use E`.
        let binding = use_expr
            .parent()
            .filter(|p| p.kind() == SyntaxKind::BINDING)?;
        let name = ast::Binding::cast(binding.clone())?
            .lhs()?
            .syntax()
            .text()
            .to_string();
        if help != format!("write `{name} <- {effect}`") {
            return None;
        }
        let text = format!("{name} <- {effect}");
        return Some((
            format!("Rewrite as `{text}`"),
            vec![(binding.text_range(), text)],
        ));
    }
    let paren = use_expr
        .parent()
        .filter(|p| p.kind() == SyntaxKind::PAREN_EXPR)?;
    let stmt = paren.ancestors().find(|n| {
        n.parent()
            .is_some_and(|p| p.kind() == SyntaxKind::BLOCK_EXPR)
    })?;
    let base = help
        .strip_prefix("write `")?
        .split(" <- ")
        .next()?
        .to_owned();
    let function = stmt
        .ancestors()
        .find(|n| matches!(n.kind(), SyntaxKind::FN_DECL | SyntaxKind::LOCAL_FN))
        .unwrap_or_else(|| root.clone());
    let taken: std::collections::HashSet<String> = function
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == SyntaxKind::LOWER_NAME)
        .map(|t| t.text().to_owned())
        .collect();
    let name = std::iter::once(base.clone())
        .chain((2..).map(|i| format!("{base}{i}")))
        .find(|n| !taken.contains(n))?;
    let text = root.text().to_string();
    let at = stmt.text_range().start();
    let line = indent(&text, at);
    // A statement that does not start its line (`with H { (use E).op() }`) gets the alias on the
    // same line, and the statement moves to the next.
    let starts_line = text[..usize::from(at)]
        .rsplit('\n')
        .next()
        .is_some_and(|before| before.chars().all(|c| matches!(c, ' ' | '\t')));
    let inserted = if starts_line {
        format!("{name} <- {effect}\n{line}")
    } else {
        format!("{name} <- {effect}\n{line}  ")
    };
    // When the statement starts with `(use E)`, one edit does both, so that they do not touch.
    let edits = if paren.text_range().start() == at {
        vec![(paren.text_range(), format!("{inserted}{name}"))]
    } else {
        vec![
            (TextRange::empty(at), inserted),
            (paren.text_range(), name.clone()),
        ]
    };
    Some((format!("Rewrite as `{name} <- {effect}`"), edits))
}

fn nearby_name(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    range: TextRange,
) -> Option<String> {
    use crate::complete::CompletionKind;

    let text = analysis.sources[file].text();
    let name = &text[range];
    let uppercase = name.chars().next()?.is_uppercase();
    let candidates: Vec<smol_str::SmolStr> =
        crate::complete::complete(program, analysis, file, range.start())
            .into_iter()
            .filter(|c| !matches!(c.kind, CompletionKind::Keyword | CompletionKind::Label))
            .filter(|c| {
                c.label
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_uppercase() == uppercase)
            })
            .map(|c| c.label.into())
            .collect();
    emela_resolve::best_match(name, &candidates).map(|s| s.to_string())
}

fn touches(target: TextRange, range: TextRange) -> bool {
    if range.is_empty() {
        target.contains(range.start()) || target == range
    } else {
        target.intersect(range).is_some_and(|r| !r.is_empty())
    }
}

fn missing_arms(
    program: &TypedProgram,
    analysis: &Analysis,
    module: ModuleId,
    range: TextRange,
    tree: &ast::SourceFile,
    text: &str,
) -> Option<Fix> {
    let missing = program.types.diags.iter().find_map(|d| {
        let span = d.span?;
        match &d.kind {
            DiagnosticKind::NonExhaustiveMatch { missing }
                if span.module == module && span.range == range =>
            {
                Some(missing)
            }
            _ => None,
        }
    })?;
    let expr = program.hir.exprs.iter().find_map(|(_, e)| {
        if e.span.module != module {
            return None;
        }
        let ExprKind::Match { scrutinee, .. } = &e.kind else {
            return None;
        };
        (program.hir.exprs[*scrutinee].span.range == range).then_some(e)
    })?;
    let node = tree
        .syntax()
        .descendants()
        .find(|n| n.kind() == SyntaxKind::MATCH_EXPR && n.text_range() == expr.span.range)?;
    let list = node
        .children()
        .find(|n| n.kind() == SyntaxKind::MATCH_ARM_LIST)?;
    let ExprKind::Match { scrutinee, .. } = expr.kind else {
        return None;
    };
    let constructors = pattern_constructors(program, program.types.exprs.get(scrutinee)?);
    let names = Names::new(program, analysis, module);
    let mut arms = missing
        .iter()
        .map(|p| {
            Some(format!(
                "{} -> todo()",
                qualify_pattern(&names, p, tree, &constructors)?
            ))
        })
        .collect::<Option<Vec<_>>>()
        .unwrap_or_else(|| vec!["_ -> todo()".to_owned()]);
    // Witnesses are examples, not necessarily an exhaustive list (spec 6.5).
    if missing.len() >= emela_types::WITNESS_LIMIT {
        arms.push("_ -> todo()".to_owned());
    }
    Some((
        "Insert missing match arms".to_owned(),
        vec![append(&list, SyntaxKind::MATCH_ARM, &arms, text)?],
    ))
}

/// Constructors reachable through the scrutinee's shape, including nested payloads.
fn pattern_constructors(program: &TypedProgram, ty: &Ty) -> Vec<DefId> {
    let mut pending = vec![ty.clone()];
    let mut seen = Vec::new();
    let mut result = Vec::new();
    while let Some(ty) = pending.pop() {
        let (def, args) = match ty {
            Ty::Named(con, args) => (program.types.ty_table.def(con), args),
            Ty::Prim(emela_types::Prim::Bool) => {
                let Some(def) = program.hir.prelude_def("Bool") else {
                    continue;
                };
                (def, Vec::new())
            }
            Ty::Tuple(elems) => {
                pending.extend(elems);
                continue;
            }
            Ty::List(elem) => {
                pending.push(*elem);
                continue;
            }
            _ => continue,
        };
        pending.extend(args.iter().cloned());
        if seen.contains(&def) {
            continue;
        }
        seen.push(def);
        let ctors = match program.hir.item(def) {
            Some(Item::Enum(e)) => e.variants.clone(),
            Some(Item::Type(t)) => t.ctor.into_iter().collect(),
            Some(Item::Handler(h)) => h.ctor.into_iter().collect(),
            Some(Item::Error(_)) => vec![def],
            _ => Vec::new(),
        };
        for &ctor in &ctors {
            if let Some(sig) = program.types.sigs.get(ctor)
                && let Ty::Fn(f) = &sig.scheme.ty
            {
                pending.extend(
                    f.params
                        .iter()
                        .map(|t| emela_types::substitute_bound(t, &args)),
                );
            }
        }
        result.extend(ctors);
    }
    result
}

/// Witnesses use bare constructor names; qualify those not imported into this file.
fn qualify_pattern(
    names: &Names<'_>,
    pattern: &str,
    tree: &ast::SourceFile,
    constructors: &[DefId],
) -> Option<String> {
    let mut result = pattern.to_owned();
    for token in emela_syntax::lex(pattern).tokens.into_iter().rev() {
        if !matches!(token.kind, SyntaxKind::UPPER_NAME | SyntaxKind::TYPE_NAME) {
            continue;
        }
        let name = &pattern[token.range];
        let mut candidates = constructors
            .iter()
            .copied()
            .filter(|&def| names.hir().defs[def].name == name);
        let def = candidates.next()?;
        let data = &names.hir().defs[def];
        // The displayed witness does not identify which of two same-named constructors it is.
        if candidates.next().is_some() {
            return None;
        }
        if names.writable(def) {
            continue;
        }
        let DefModule::Source(module) = data.module else {
            return None;
        };
        let path = names.qualifier(module, tree)?;
        result.replace_range(
            std::ops::Range::<usize>::from(token.range),
            &format!("{}.{name}", path.last()),
        );
    }
    Some(result)
}

fn extend_annotation(
    program: &TypedProgram,
    analysis: &Analysis,
    module: ModuleId,
    diagnostic: &Diagnostic,
    tree: &ast::SourceFile,
) -> Option<Fix> {
    let Some(Location::Span(span)) = diagnostic.location else {
        return None;
    };
    let effects = diagnostic.code == Some(code::UNDECLARED_EFFECTS);
    // main, tests, constants, and handler operations cannot propagate effects this way, and the
    // same codes are given to a function value whose E or R does not fit, which has no clause to
    // extend.
    if !program.types.diags.iter().any(|d| {
        d.span
            .is_some_and(|s| s.module == module && s.range == span.range)
            && if effects {
                matches!(
                    d.kind,
                    DiagnosticKind::UnhandledEffects {
                        context: EffectContext::Declared,
                        ..
                    }
                )
            } else {
                matches!(d.kind, DiagnosticKind::UnhandledErrors { .. })
            }
    }) {
        return None;
    }
    let f = tree
        .fn_decls()
        .find(|f| f.syntax().text_range().contains_range(span.range))?;
    let name = f.name()?;
    let def = program.hir.defs.iter().find_map(|(def, data)| {
        data.span
            .is_some_and(|s| s.module == module && s.range == name.text_range())
            .then_some(def)
    })?;
    let Item::Fn(item) = program.hir.item(def)? else {
        return None;
    };
    let body = item.body?;
    let sig = program.types.sigs.get(def)?;
    let Ty::Fn(ty) = &sig.scheme.ty else {
        return None;
    };
    let (body_row, declared_row, kind, clause) = if effects {
        (
            program.types.effects.get(body)?,
            &ty.uses,
            SyntaxKind::USES_CLAUSE,
            "uses",
        )
    } else {
        (
            program.types.errors.get(body)?,
            &ty.fails,
            SyntaxKind::FAILS_CLAUSE,
            "fails",
        )
    };
    let row = visible(
        &body_row.union(declared_row),
        item.sig.type_params.len() as u32,
    );
    if row.is_empty() || !Names::new(program, analysis, module).writable_row(&row) {
        return None;
    }
    let cons = &program.types.ty_table.cons;
    let params = &sig.scheme.params;
    let value = if effects {
        row.display_uses(cons, params).to_string()
    } else {
        row.display_with(cons, params).to_string()
    };
    let replacement = format!("{clause} {value}");
    let edit = clause_edit(&f, kind, &replacement)?;
    Some((format!("Add missing {clause}: `{replacement}`"), vec![edit]))
}

fn imports(
    program: &TypedProgram,
    analysis: &Analysis,
    module: ModuleId,
    diagnostic: &Diagnostic,
    range: TextRange,
    tree: &ast::SourceFile,
) -> Vec<CodeAction> {
    let text = analysis.sources[*analysis.files.get(module).unwrap()].text();
    let name = &text[range];
    let qualified = text[usize::from(range.end())..]
        .trim_start()
        .starts_with('.');
    let existing: Vec<_> = tree
        .imports()
        .filter_map(|i| {
            let p = i.path()?;
            Some(emela_resolve::ModuleName::new(
                p.segments().map(|t| t.text().to_owned()),
            ))
        })
        .collect();
    let at = tree.imports().last().map(|i| i.syntax().text_range().end());
    let mut paths = Vec::new();
    if qualified {
        for (target, data) in analysis.modules.iter() {
            if target != module
                && data.name.last() == name
                && analysis.modules.lookup_from(module, &data.name) == Some(target)
                && !existing.contains(&data.name)
            {
                paths.push(data.name.to_string());
            }
        }
    } else {
        for (_, data) in program.hir.defs.iter() {
            let DefModule::Source(target) = data.module else {
                continue;
            };
            if target == module || data.name != name || !data.is_pub || data.parent.is_some() {
                continue;
            }
            let path = &analysis.modules[target].name;
            if analysis.modules.lookup_from(module, path) == Some(target) {
                paths.push(format!("{path}.{{{name}}}"));
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .map(|path| {
            let (at, inserted) = match at {
                Some(at) => (at, format!("\nimport {path}")),
                None => (TextSize::from(0), format!("import {path}\n")),
            };
            fix(
                format!("Import `{path}`"),
                diagnostic,
                vec![(TextRange::empty(at), inserted)],
            )
        })
        .collect()
}
