//! Diagnostic-linked edits. All locations refer to the current analysis snapshot.

use emela_resolve::ModuleId;
use emela_resolve::hir::{DefId, DefModule, ExprKind, Item};
use emela_syntax::ast::{self, AstNode};
use emela_syntax::{DiagnosticCode, SyntaxKind, SyntaxNode, rewrite};
use emela_types::{DiagnosticKind, EffectContext, Ty};
use line_index::{TextRange, TextSize};

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
            Some(code::UNHANDLED_ERRORS | code::UNHANDLED_EFFECTS) => {
                extend_annotation(program, analysis, module, diagnostic, &tree)
            }
            Some(c) if c == DiagnosticCode::OldImplHead.as_str() => {
                old_impl_head(tree.syntax(), span.range)
            }
            Some(c) if c == DiagnosticCode::OldHandlerHead.as_str() => {
                old_handler_head(tree.syntax(), span.range)
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

type Edit = (TextRange, String);
pub(super) type Fix = (String, Vec<Edit>);

fn fix(title: String, diagnostic: &Diagnostic, edits: Vec<Edit>) -> CodeAction {
    CodeAction {
        title,
        kind: CodeActionKind::QuickFix,
        diagnostics: vec![diagnostic.clone()],
        edits,
    }
}

/// `impl Show for User` -> `impl User: Show` (E0144, spec 10.2).
fn old_impl_head(root: &SyntaxNode, range: TextRange) -> Option<Fix> {
    let (range, head) = root
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::IMPL_DECL)
        .find_map(|n| rewrite::old_impl_head(&n).filter(|(r, _)| *r == range))?;
    Some((format!("Rewrite as `{head}`"), vec![(range, head)]))
}

/// `handler H implements E` -> `handler H: E` (E0145, spec 8.4).
fn old_handler_head(root: &SyntaxNode, range: TextRange) -> Option<Fix> {
    let (_, edit, head) = root
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::HANDLER_DECL)
        .find_map(|n| rewrite::old_handler_head(&n).filter(|(t, _, _)| t.text_range() == range))?;
    Some((format!("Rewrite as `{head}`"), vec![(edit, ":".to_owned())]))
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
        let path = &names.analysis.modules[module].name;
        let available = (names.analysis.modules.is_core(module) && path.segments().len() == 1)
            || tree.imports().filter_map(|i| i.path()).any(|p| {
                let imported =
                    emela_resolve::ModuleName::new(p.segments().map(|t| t.text().to_owned()));
                names.analysis.modules.lookup_from(names.module, &imported) == Some(module)
            });
        if !available {
            return None;
        }
        result.replace_range(
            std::ops::Range::<usize>::from(token.range),
            &format!("{}.{name}", path.last()),
        );
    }
    Some(result)
}

/// Append after the last arm/method, preserving trailing comments and the closing brace.
pub(super) fn append(
    list: &SyntaxNode,
    kind: SyntaxKind,
    items: &[String],
    text: &str,
) -> Option<Edit> {
    let last = list.children().filter(|n| n.kind() == kind).last();
    let open = list
        .children_with_tokens()
        .find(|e| e.kind() == SyntaxKind::L_BRACE)?;
    let mut at = last
        .as_ref()
        .map_or(open.text_range().end(), |n| n.text_range().end());
    let tail = &text[usize::from(at)..usize::from(list.text_range().end())];
    let line = tail.split('\n').next()?.trim_end_matches('\r');
    if line.trim_start().starts_with('#') {
        at += TextSize::try_from(line.len()).ok()?;
    }
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let base = indent(text, list.text_range().start());
    let inner = last
        .as_ref()
        .map(|n| indent(text, n.text_range().start()))
        .filter(|s| s.len() > base.len())
        .unwrap_or_else(|| format!("{base}  "));
    let mut inserted = items
        .iter()
        .map(|item| format!("{newline}{inner}{item}"))
        .collect::<String>();
    // An inline block needs a newline before its closing brace as well.
    let tail = &text[usize::from(at)..usize::from(list.text_range().end())];
    if !tail.trim_start_matches([' ', '\t']).starts_with('\n')
        && !tail.trim_start_matches([' ', '\t']).starts_with("\r\n")
    {
        inserted.push_str(&format!("{newline}{base}"));
    }
    Some((TextRange::empty(at), inserted))
}

fn indent(text: &str, at: TextSize) -> String {
    text[..usize::from(at)]
        .rsplit('\n')
        .next()
        .unwrap_or("")
        .chars()
        .take_while(|c| matches!(c, ' ' | '\t'))
        .collect()
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
    let effects = diagnostic.code == Some(code::UNHANDLED_EFFECTS);
    // main, tests, constants, and handler operations cannot propagate effects this way.
    if effects
        && !program.types.diags.iter().any(|d| {
            d.span
                .is_some_and(|s| s.module == module && s.range == span.range)
                && matches!(
                    d.kind,
                    DiagnosticKind::UnhandledEffects {
                        context: EffectContext::Declared,
                        ..
                    }
                )
        })
    {
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
            SyntaxKind::USE_CLAUSE,
            "use",
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
        row.display_effects(cons, params).to_string()
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
