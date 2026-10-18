//! Checks for fns visible only inside the bundled core (`@internal`).
//!
//! `@internal` may only be put on top-level fns in bundled core sources (E0229), and such fns
//! cannot be referenced from outside core modules (E0228). It is for fns that only core should
//! use, such as passing the contents of an opaque type to another core module. No fn in the
//! current bundled core uses it.

use emela_resolve::ModuleId;
use emela_resolve::hir::{DefId, DefKind, DefModule, Program};
use emela_syntax::ast::{self, AstNode, HasAttrs};
use emela_syntax::{Parse, SyntaxKind};
use line_index::TextRange;

use crate::code;
use crate::diagnostic::{Diagnostic, HELP_PREFIX, Span};
use crate::pipeline::Analysis;

pub const INTERNAL_ANNOTATION: &str = "internal";

/// Checks where `@internal` is placed (E0229) and reports references to `@internal` fns from
/// outside core as E0228. References include uses as values, not just calls.
pub(crate) fn check_internal<'p>(
    analysis: &Analysis,
    hir: &Program,
    parse_of: impl Fn(ModuleId) -> Option<&'p Parse>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    // Module and name range of each `@internal` fn (same as the resolver's `DefData::span`).
    let mut names: Vec<(ModuleId, TextRange)> = Vec::new();
    for (module, _) in analysis.modules.iter() {
        let (Some(&file), Some(parse)) = (analysis.files.get(module), parse_of(module)) else {
            continue;
        };
        let tree = parse.tree();
        for annotation in tree
            .syntax()
            .descendants()
            .filter_map(ast::Annotation::cast)
        {
            if annotation
                .name()
                .is_none_or(|name| name.text() != INTERNAL_ANNOTATION)
            {
                continue;
            }
            let range = annotation.syntax().text_range();
            let error = |message: &str| {
                Diagnostic::error(message)
                    .with_code(code::MISPLACED_INTERNAL)
                    .with_span(Span::new(file, range))
            };
            if !analysis.modules.is_core(module) {
                diagnostics.push(error(
                    "`@internal` can only be used in the bundled core library",
                ));
                continue;
            }
            let top_level_fn = annotation
                .syntax()
                .parent()
                .filter(|p| p.parent().is_some_and(|pp| pp.kind() == SyntaxKind::ROOT))
                .and_then(ast::FnDecl::cast);
            let Some(name) = top_level_fn.and_then(|decl| decl.name()) else {
                diagnostics.push(error("`@internal` can only be used on top-level functions"));
                continue;
            };
            names.push((module, name.text_range()));
        }
    }

    let internal: Vec<DefId> = hir
        .defs
        .iter()
        .filter(|(_, data)| {
            data.kind == DefKind::Fn
                && data
                    .span
                    .is_some_and(|span| names.contains(&(span.module, span.range)))
        })
        .map(|(def, _)| def)
        .collect();
    if internal.is_empty() {
        return diagnostics;
    }
    for (def, data) in hir.defs.iter() {
        let DefModule::Source(module) = data.module else {
            continue;
        };
        if analysis.modules.is_core(module) {
            continue;
        }
        for (target, span) in emela_types::body_refs(hir, def) {
            if !internal.contains(&target) {
                continue;
            }
            let Some(&file) = analysis.files.get(span.module) else {
                continue;
            };
            let target = &hir.defs[target];
            let name = match target.module {
                DefModule::Source(m) => format!("{}.{}", analysis.modules[m].name, target.name),
                _ => target.name.to_string(),
            };
            diagnostics.push(
                Diagnostic::error(format!(
                    "function `{name}` can only be used from the bundled core library"
                ))
                .with_code(code::INTERNAL_FN_OUTSIDE_CORE)
                .with_span(Span::new(file, span.range))
                .with_note("it is marked `@internal`"),
            );
        }
    }
    diagnostics
}

/// `@external` in the modules of a dependency (spec 4.7): only in a Pome that `[external] allow`
/// of the project allows (E0233), and never on a `pub` fn (E0237), so that the host is reached
/// only through the Pome's handlers.
pub(crate) fn check_external_in_dependencies<'p>(
    analysis: &Analysis,
    parse_of: impl Fn(ModuleId) -> Option<&'p Parse>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (module, _) in analysis.modules.iter() {
        let Some(package) = analysis.modules.package_of(module) else {
            continue;
        };
        let (Some(&file), Some(parse)) = (analysis.files.get(module), parse_of(module)) else {
            continue;
        };
        for annotation in parse
            .tree()
            .syntax()
            .descendants()
            .filter_map(ast::Annotation::cast)
        {
            if annotation
                .name()
                .is_none_or(|name| name.text() != "external")
            {
                continue;
            }
            let span = Span::new(file, annotation.syntax().text_range());
            if !package.external_allowed {
                diagnostics.push(
                    Diagnostic::error(format!("`{}` uses `@external`, but it is not allowed", package.name))
                        .with_code(code::EXTERNAL_IN_DEPENDENCY)
                        .with_span(span)
                        .with_note(format!(
                            "{HELP_PREFIX}add it to `[external] allow` in Pome.toml, or run `emela pome add --allow-external {}`",
                            package.name
                        )),
                );
                // One error per module is enough to say the Pome needs allowing.
                break;
            }
            let fn_decl = annotation.syntax().parent().and_then(ast::FnDecl::cast);
            if fn_decl.is_some_and(|f| f.is_pub()) {
                diagnostics.push(
                    Diagnostic::error("an `@external` function cannot be `pub` in a dependency")
                        .with_code(code::PUB_EXTERNAL_IN_DEPENDENCY)
                        .with_span(span)
                        .with_note(format!(
                            "`{}` reaches the host through its handlers; expose an effect, or a function that calls this one",
                            package.name
                        )),
                );
            }
        }
    }
    diagnostics
}
