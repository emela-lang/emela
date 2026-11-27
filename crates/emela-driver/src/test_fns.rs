//! The `@test` functions of the source (spec 13.1): collecting them, and resolving them to
//! their definitions.

use emela_resolve::ModuleId;
use emela_resolve::hir::{DefId, DefKind, Program};
use emela_syntax::ast::{self, AstNode, HasAttrs};
use emela_syntax::{Parse, SyntaxKind};
use line_index::TextRange;

use crate::analysis::Analysis;
use crate::code;
use crate::diagnostic::{Diagnostic, Span};
use crate::source::FileId;

/// The annotation name of test functions.
pub const TEST_ANNOTATION: &str = "test";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestFn {
    pub module: ModuleId,
    pub name: String,
    /// The name used for display and filtering: just the function name in the entry module, and
    /// prefixed with the module name elsewhere, like `Http.Client.name`.
    pub display_name: String,
    pub file: FileId,
    /// The range of the function's name token, the same range as the resolver's `DefData::span`,
    /// so the HIR definition can be looked up.
    pub name_range: TextRange,
}

/// Collects `@test` functions from the trees of read modules, in module order and then
/// declaration order.
///
/// `@test` may only be put on top-level fns without parameters that have a body (not
/// `@external`). `@test` elsewhere, and arguments like `@test(...)`, are E2203. Functions with
/// errors are not collected.
pub fn collect_tests<'p>(
    analysis: &Analysis,
    parse_of: impl Fn(ModuleId) -> Option<&'p Parse>,
) -> (Vec<TestFn>, Vec<Diagnostic>) {
    let mut tests = Vec::new();
    let mut diagnostics = Vec::new();
    for (module, data) in analysis.modules.iter() {
        // The tests of dependencies are theirs to run, not the project's.
        if analysis.modules.is_dependency(module) {
            continue;
        }
        let (Some(&file), Some(parse)) = (analysis.files.get(module), parse_of(module)) else {
            continue;
        };
        let error = |message: String, range: TextRange| {
            Diagnostic::error(message)
                .with_code(code::INVALID_TEST)
                .with_span(Span::new(file, range))
        };
        let tree = parse.tree();
        for annotation in tree
            .syntax()
            .descendants()
            .filter_map(ast::Annotation::cast)
        {
            if annotation
                .name()
                .is_none_or(|name| name.text() != TEST_ANNOTATION)
            {
                continue;
            }
            let range = annotation.syntax().text_range();
            // Even empty parentheses like `@test()` form an argument list.
            if annotation
                .syntax()
                .children()
                .any(|n| n.kind() == SyntaxKind::ANNOT_ARG_LIST)
            {
                diagnostics.push(error("`@test` takes no arguments".into(), range));
                continue;
            }
            // On a local fn, E1212 already reports it (`[fn.local]`).
            if annotation
                .syntax()
                .parent()
                .is_some_and(|p| p.kind() == SyntaxKind::LOCAL_FN)
            {
                continue;
            }
            let top_level_fn = annotation
                .syntax()
                .parent()
                .filter(|p| p.parent().is_some_and(|pp| pp.kind() == SyntaxKind::ROOT))
                .and_then(ast::FnDecl::cast);
            let Some(decl) = top_level_fn else {
                diagnostics.push(
                    error(
                        "`@test` can only be used on top-level functions".into(),
                        range,
                    )
                    .with_note("a test is a function without parameters (13.1)"),
                );
                continue;
            };
            // Two `@test`s on one function collect it once.
            if decl
                .annotations()
                .find(|a| a.name().is_some_and(|n| n.text() == TEST_ANNOTATION))
                .is_some_and(|first| first.syntax() != annotation.syntax())
            {
                continue;
            }
            let Some(name) = decl.name() else {
                // A declaration without a name already has a syntax diagnostic.
                continue;
            };
            if let Some(params) = decl.param_list()
                && params.params().next().is_some()
            {
                diagnostics.push(error(
                    format!("test function `{}` cannot take parameters", name.text()),
                    params.syntax().text_range(),
                ));
                continue;
            }
            if decl.body().is_none() {
                // A missing body itself is handled by the syntax stage (E1204) or as `@external`.
                diagnostics.push(error(
                    format!("test function `{}` needs a body", name.text()),
                    name.text_range(),
                ));
                continue;
            }
            let display_name = if analysis.entry == Some(module) {
                name.text().to_owned()
            } else {
                format!("{}.{}", data.name, name.text())
            };
            tests.push(TestFn {
                module,
                name: name.text().to_owned(),
                display_name,
                file,
                name_range: name.text_range(),
            });
        }
    }
    (tests, diagnostics)
}

/// Whether a fn declaration has `@test`. Lowering uses it to drop test functions from the normal
/// build (spec 13.1).
pub fn is_test_fn(decl: &ast::FnDecl) -> bool {
    decl.has_annotation(TEST_ANNOTATION)
}

/// A `@test` function that passed type checking. `def` is its HIR definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestDef {
    pub test: TestFn,
    pub def: DefId,
}

/// Maps collected `@test` functions to HIR definitions, and reports references to test functions
/// from normal code as E2204. Test functions are removed from the normal build (spec 13.1), so
/// they cannot be referenced. References between test functions are allowed.
pub(crate) fn resolve_tests(
    analysis: &Analysis,
    hir: &Program,
    tests: Vec<TestFn>,
) -> (Vec<TestDef>, Vec<Diagnostic>) {
    // `TestFn::name_range` is the same range as the resolver's `DefData::span`.
    let tests: Vec<TestDef> = tests
        .into_iter()
        .filter_map(|test| {
            let def = hir.defs.iter().find_map(|(def, data)| {
                let span = data.span?;
                (data.kind == DefKind::Fn
                    && span.module == test.module
                    && span.range == test.name_range)
                    .then_some(def)
            })?;
            Some(TestDef { test, def })
        })
        .collect();

    let mut diagnostics = Vec::new();
    for (def, _) in hir.defs.iter() {
        if tests.iter().any(|t| t.def == def) {
            continue;
        }
        for (target, span) in emela_types::body_refs(hir, def) {
            if !tests.iter().any(|t| t.def == target) {
                continue;
            }
            let Some(&file) = analysis.files.get(span.module) else {
                continue;
            };
            diagnostics.push(
                Diagnostic::error(format!(
                    "test function `{}` can only be used from test functions",
                    hir.defs[target].name
                ))
                .with_code(code::TEST_FN_OUTSIDE_TESTS)
                .with_span(crate::diagnostic::Span::new(file, span.range))
                .with_note("test functions are removed from normal builds (13.1)"),
            );
        }
    }
    (tests, diagnostics)
}
