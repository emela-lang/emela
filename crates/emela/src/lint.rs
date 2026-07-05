//! The linter, `emela lint` (spec 0035): a fixed set of warning rules over a
//! program that already parses and type-checks. There is no configuration
//! and no suppression file; every finding carries a rule id
//! (`warning: <title> [<rule/id>]`), all findings are collected and printed
//! in source order, and any finding makes the exit code non-zero.
//!
//! The syntax rules (naming, unused imports/bindings) walk the root file's
//! own AST, so imported modules and the prelude are never reported. The
//! effect rule compares each declared `uses { ... }` row with the row the
//! type checker computed for the body.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::ast::{self, Type};
use crate::error::{Diagnostic, Error, Result, Span};
use crate::parser::parse_program;
use crate::typecheck::TypedProgram;

/// Lints every input file (spec 0035 L1-L5). Exit code 0 with no output when
/// clean; 1 with all findings on stderr otherwise.
pub(crate) fn run(inputs: &[PathBuf], packages: &[PathBuf]) -> Result<()> {
    let mut findings = 0usize;
    let mut failed = 0usize;
    for input in inputs {
        match lint_file(input, packages) {
            Ok(diagnostics) => {
                for diagnostic in &diagnostics {
                    eprintln!("{}", diagnostic.render());
                    eprintln!();
                }
                findings += diagnostics.len();
            }
            // A file that does not parse or type-check reports that error
            // instead of lint findings (L1).
            Err(error) => {
                eprintln!("{error}");
                eprintln!();
                failed += 1;
            }
        }
    }
    if failed > 0 {
        return Err(Error::new(format!("{failed} file(s) failed to lint")));
    }
    if findings > 0 {
        let plural = if findings == 1 { "" } else { "s" };
        return Err(Error::new(format!("{findings} warning{plural} emitted")));
    }
    Ok(())
}

fn lint_file(input: &Path, packages: &[PathBuf]) -> Result<Vec<Diagnostic>> {
    let label = input.display().to_string();
    let source = fs::read_to_string(input)
        .map_err(|error| Error::new(format!("failed to read `{}`: {error}", input.display())))?;
    // The root file's own AST, before imports and the prelude are merged in:
    // the syntax rules must only see the user's declarations (L2). Linting
    // requires a clean parse; the first collected error (spec 0033) is surfaced.
    let (root, errors) = parse_program(&label, &source);
    if let Some(error) = errors.into_iter().next() {
        return Err(error);
    }
    // The full frontend must succeed before lints are reported; `main` is not
    // required — libraries are lintable (L1).
    let (merged, typed) = crate::driver::compile_frontend(&input.to_path_buf(), packages, false)?;
    let mut diagnostics = Vec::new();
    naming(&root, &mut diagnostics);
    unused_imports(&root, &mut diagnostics);
    unused_bindings(&root, &mut diagnostics);
    over_declared_effects(&merged, &typed, &label, &mut diagnostics);
    diagnostics.sort_by_key(|diagnostic| diagnostic.span().map_or(0, |span| span.start));
    Ok(diagnostics)
}

// ---------------------------------------------------------------------------
// naming/snake-case, naming/pascal-case
// ---------------------------------------------------------------------------

fn is_snake_case(name: &str) -> bool {
    !name.contains(char::is_uppercase)
}

fn is_pascal_case(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|first| first.is_ascii_uppercase())
        && !name.contains('_')
}

fn warn_snake(name: &str, span: &Span, what: &str, out: &mut Vec<Diagnostic>) {
    if !is_snake_case(name) {
        out.push(
            Diagnostic::warning(format!("{what} `{name}` is not snake_case"))
                .code("naming/snake-case")
                .label(span.clone(), "identifiers use snake_case"),
        );
    }
}

fn warn_pascal(name: &str, span: &Span, what: &str, out: &mut Vec<Diagnostic>) {
    if !is_pascal_case(name) {
        out.push(
            Diagnostic::warning(format!("{what} `{name}` is not PascalCase"))
                .code("naming/pascal-case")
                .label(span.clone(), "types and enum variants use PascalCase"),
        );
    }
}

fn naming(program: &ast::Program, out: &mut Vec<Diagnostic>) {
    for function in &program.functions {
        warn_snake(&function.name, &function.name_span, "function", out);
        naming_signature_and_body(&function.params, Some(&function.body), out);
    }
    for declaration in &program.externs {
        warn_snake(&declaration.name, &declaration.name_span, "function", out);
        naming_signature_and_body(&declaration.params, None, out);
    }
    for declaration in &program.enums {
        warn_pascal(&declaration.name, &declaration.name_span, "enum", out);
        for variant in &declaration.variants {
            warn_pascal(&variant.name, &variant.name_span, "variant", out);
        }
    }
    for declaration in &program.traits {
        warn_pascal(&declaration.name, &declaration.name_span, "trait", out);
        for method in &declaration.methods {
            warn_snake(&method.name, &method.name_span, "function", out);
            naming_signature_and_body(&method.params, method.default_body.as_ref(), out);
        }
    }
    for declaration in &program.impls {
        // Method names are fixed by the trait (already linted there); the
        // parameter and binding names are the impl author's.
        for method in &declaration.methods {
            naming_signature_and_body(&method.params, Some(&method.body), out);
        }
    }
}

fn naming_signature_and_body(
    params: &[ast::Param],
    body: Option<&ast::Block>,
    out: &mut Vec<Diagnostic>,
) {
    for param in params {
        warn_snake(&param.name, &param.name_span, "parameter", out);
    }
    if let Some(body) = body {
        naming_block(body, out);
    }
}

fn naming_block(block: &ast::Block, out: &mut Vec<Diagnostic>) {
    for item in &block.items {
        match item {
            ast::BlockItem::Let {
                name,
                name_span,
                value,
                ..
            } => {
                warn_snake(name, name_span, "binding", out);
                naming_expr(value, out);
            }
            ast::BlockItem::Expr(expr) => naming_expr(expr, out),
        }
    }
}

fn naming_expr(expr: &ast::Expr, out: &mut Vec<Diagnostic>) {
    use ast::Expr;
    match expr {
        Expr::Fn { params, body, .. } => {
            for param in params {
                warn_snake(&param.name, &param.name_span, "parameter", out);
            }
            naming_block(body, out);
        }
        Expr::Array(items, _) => items.iter().for_each(|item| naming_expr(item, out)),
        Expr::Call { callee, args, .. } => {
            naming_expr(callee, out);
            args.iter().for_each(|arg| naming_expr(arg, out));
        }
        Expr::Binary { left, right, .. } => {
            naming_expr(left, out);
            naming_expr(right, out);
        }
        Expr::Block(block) => naming_block(block, out),
        Expr::If {
            cond, then, els, ..
        } => {
            naming_expr(cond, out);
            naming_block(then, out);
            naming_block(els, out);
        }
        Expr::Throw { value, .. } | Expr::Question { value, .. } => naming_expr(value, out),
        Expr::Panic { message, .. } => naming_expr(message, out),
        Expr::Match {
            scrutinee, arms, ..
        } => {
            naming_expr(scrutinee, out);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    naming_expr(guard, out);
                }
                naming_expr(&arm.body, out);
            }
        }
        Expr::Try { body, arms, .. } => {
            naming_block(body, out);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    naming_expr(guard, out);
                }
                naming_expr(&arm.body, out);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// imports/unused
// ---------------------------------------------------------------------------

/// Conservative: an import is unused only when its item name appears nowhere
/// in the file — not as a value, a path segment, a type name, or a pattern.
fn unused_imports(program: &ast::Program, out: &mut Vec<Diagnostic>) {
    let used = used_names(program);
    for import in &program.imports {
        let name = import.item_name();
        if !used.contains(name) {
            out.push(
                Diagnostic::warning("Unused import")
                    .code("imports/unused")
                    .label(
                        import.span.clone(),
                        format!("`{name}` is imported but never used"),
                    ),
            );
        }
    }
}

fn used_names(program: &ast::Program) -> HashSet<String> {
    let mut used = HashSet::new();
    for function in &program.functions {
        used_signature(&function.params, &function.ret, &function.throws, &mut used);
        used_block(&function.body, &mut used);
    }
    for declaration in &program.externs {
        used_signature(
            &declaration.params,
            &declaration.ret,
            &declaration.throws,
            &mut used,
        );
    }
    for declaration in &program.enums {
        for variant in &declaration.variants {
            for field in &variant.fields {
                used_type(field, &mut used);
            }
        }
    }
    for declaration in &program.traits {
        for method in &declaration.methods {
            used_signature(&method.params, &method.ret, &method.throws, &mut used);
            if let Some(body) = &method.default_body {
                used_block(body, &mut used);
            }
        }
    }
    for declaration in &program.impls {
        used.insert(declaration.trait_name.clone());
        used_type(&declaration.target, &mut used);
        for method in &declaration.methods {
            used_signature(&method.params, &method.ret, &method.throws, &mut used);
            used_block(&method.body, &mut used);
        }
    }
    used
}

fn used_signature(
    params: &[ast::Param],
    ret: &Type,
    throws: &Option<Type>,
    used: &mut HashSet<String>,
) {
    for param in params {
        used_type(&param.ty, used);
    }
    used_type(ret, used);
    if let Some(throws) = throws {
        used_type(throws, used);
    }
}

fn used_type(ty: &Type, used: &mut HashSet<String>) {
    match ty {
        Type::Enum(name, args) => {
            used.insert(name.clone());
            args.iter().for_each(|arg| used_type(arg, used));
        }
        Type::Array(inner) | Type::Option(inner) => used_type(inner, used),
        Type::Function(function) => {
            function
                .params
                .iter()
                .for_each(|param| used_type(param, used));
            used_type(&function.ret, used);
            if let Some(throws) = &function.throws {
                used_type(throws, used);
            }
        }
        _ => {}
    }
}

fn used_block(block: &ast::Block, used: &mut HashSet<String>) {
    for item in &block.items {
        match item {
            ast::BlockItem::Let { ty, value, .. } => {
                if let Some(ty) = ty {
                    used_type(ty, used);
                }
                used_expr(value, used);
            }
            ast::BlockItem::Expr(expr) => used_expr(expr, used),
        }
    }
}

fn used_expr(expr: &ast::Expr, used: &mut HashSet<String>) {
    use ast::Expr;
    match expr {
        Expr::Var(name, _) => {
            used.insert(name.clone());
        }
        Expr::Path { segments, .. } | Expr::TypePath { segments, .. } => {
            for segment in segments {
                used.insert(segment.clone());
            }
        }
        Expr::Array(items, _) => items.iter().for_each(|item| used_expr(item, used)),
        Expr::Call { callee, args, .. } => {
            used_expr(callee, used);
            args.iter().for_each(|arg| used_expr(arg, used));
        }
        Expr::Fn {
            params, ret, body, ..
        } => {
            for param in params {
                used_type(&param.ty, used);
            }
            used_type(ret, used);
            used_block(body, used);
        }
        Expr::Binary { left, right, .. } => {
            used_expr(left, used);
            used_expr(right, used);
        }
        Expr::Block(block) => used_block(block, used),
        Expr::If {
            cond, then, els, ..
        } => {
            used_expr(cond, used);
            used_block(then, used);
            used_block(els, used);
        }
        Expr::Throw { value, .. } | Expr::Question { value, .. } => used_expr(value, used),
        Expr::Panic { message, .. } => used_expr(message, used),
        Expr::Match {
            scrutinee, arms, ..
        } => {
            used_expr(scrutinee, used);
            used_arms(arms, used);
        }
        Expr::Try { body, arms, .. } => {
            used_block(body, used);
            used_arms(arms, used);
        }
        _ => {}
    }
}

fn used_arms(arms: &[ast::MatchArm], used: &mut HashSet<String>) {
    for arm in arms {
        if let ast::Pattern::Variant {
            enum_name, variant, ..
        } = &arm.pattern
        {
            if let Some(enum_name) = enum_name {
                used.insert(enum_name.clone());
            }
            used.insert(variant.clone());
        }
        if let Some(guard) = &arm.guard {
            used_expr(guard, used);
        }
        used_expr(&arm.body, used);
    }
}

// ---------------------------------------------------------------------------
// bindings/unused-let, bindings/unused-param
// ---------------------------------------------------------------------------

/// A binding or parameter is unused when its name is never read anywhere in
/// the enclosing body. Whole-body counting is deliberately conservative
/// about shadowing: a shadowed-but-unused binding may go unreported, but a
/// used one is never flagged. `_`-prefixed names are exempt.
fn unused_bindings(program: &ast::Program, out: &mut Vec<Diagnostic>) {
    for function in &program.functions {
        unused_in_function(&function.params, &function.body, true, out);
    }
    for declaration in &program.traits {
        for method in &declaration.methods {
            // Trait signatures fix the parameter list, so a default body's
            // unused parameters are not the author's choice: lets only.
            if let Some(body) = &method.default_body {
                unused_in_function(&[], body, false, out);
            }
        }
    }
    for declaration in &program.impls {
        for method in &declaration.methods {
            // Same exemption for impl methods (spec 0035): lets only.
            unused_in_function(&[], &method.body, false, out);
        }
    }
}

fn unused_in_function(
    params: &[ast::Param],
    body: &ast::Block,
    check_params: bool,
    out: &mut Vec<Diagnostic>,
) {
    let mut reads = HashSet::new();
    reads_block(body, &mut reads);
    if check_params {
        for param in params {
            if !param.name.starts_with('_') && !reads.contains(&param.name) {
                out.push(
                    Diagnostic::warning("Unused parameter")
                        .code("bindings/unused-param")
                        .label(
                            param.name_span.clone(),
                            format!("`{}` is never used", param.name),
                        )
                        .help("Prefix the name with `_` to keep it intentionally."),
                );
            }
        }
    }
    unused_lets_in_block(body, &reads, out);
}

/// Collects every name that is *read* (as a value) in a block.
fn reads_block(block: &ast::Block, reads: &mut HashSet<String>) {
    for item in &block.items {
        match item {
            ast::BlockItem::Let { value, .. } => reads_expr(value, reads),
            ast::BlockItem::Expr(expr) => reads_expr(expr, reads),
        }
    }
}

fn reads_expr(expr: &ast::Expr, reads: &mut HashSet<String>) {
    use ast::Expr;
    match expr {
        Expr::Var(name, _) => {
            reads.insert(name.clone());
        }
        // The head of a dotted path may be a local (a receiver call,
        // spec 0018); count every segment to stay conservative.
        Expr::Path { segments, .. } => {
            for segment in segments {
                reads.insert(segment.clone());
            }
        }
        Expr::Array(items, _) => items.iter().for_each(|item| reads_expr(item, reads)),
        Expr::Call { callee, args, .. } => {
            reads_expr(callee, reads);
            args.iter().for_each(|arg| reads_expr(arg, reads));
        }
        Expr::Fn { body, .. } => reads_block(body, reads),
        Expr::Binary { left, right, .. } => {
            reads_expr(left, reads);
            reads_expr(right, reads);
        }
        Expr::Block(block) => reads_block(block, reads),
        Expr::If {
            cond, then, els, ..
        } => {
            reads_expr(cond, reads);
            reads_block(then, reads);
            reads_block(els, reads);
        }
        Expr::Throw { value, .. } | Expr::Question { value, .. } => reads_expr(value, reads),
        Expr::Panic { message, .. } => reads_expr(message, reads),
        Expr::Match {
            scrutinee, arms, ..
        } => {
            reads_expr(scrutinee, reads);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    reads_expr(guard, reads);
                }
                reads_expr(&arm.body, reads);
            }
        }
        Expr::Try { body, arms, .. } => {
            reads_block(body, reads);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    reads_expr(guard, reads);
                }
                reads_expr(&arm.body, reads);
            }
        }
        _ => {}
    }
}

fn unused_lets_in_block(block: &ast::Block, reads: &HashSet<String>, out: &mut Vec<Diagnostic>) {
    for item in &block.items {
        match item {
            ast::BlockItem::Let {
                name,
                name_span,
                value,
                ..
            } => {
                if !name.starts_with('_') && !reads.contains(name) {
                    out.push(
                        Diagnostic::warning("Unused binding")
                            .code("bindings/unused-let")
                            .label(name_span.clone(), format!("`{name}` is never used"))
                            .help("Prefix the name with `_` to keep it intentionally."),
                    );
                }
                unused_lets_in_expr(value, reads, out);
            }
            ast::BlockItem::Expr(expr) => unused_lets_in_expr(expr, reads, out),
        }
    }
}

fn unused_lets_in_expr(expr: &ast::Expr, reads: &HashSet<String>, out: &mut Vec<Diagnostic>) {
    use ast::Expr;
    match expr {
        Expr::Fn { body, .. } => unused_lets_in_block(body, reads, out),
        Expr::Array(items, _) => items
            .iter()
            .for_each(|item| unused_lets_in_expr(item, reads, out)),
        Expr::Call { callee, args, .. } => {
            unused_lets_in_expr(callee, reads, out);
            args.iter()
                .for_each(|arg| unused_lets_in_expr(arg, reads, out));
        }
        Expr::Binary { left, right, .. } => {
            unused_lets_in_expr(left, reads, out);
            unused_lets_in_expr(right, reads, out);
        }
        Expr::Block(block) => unused_lets_in_block(block, reads, out),
        Expr::If {
            cond, then, els, ..
        } => {
            unused_lets_in_expr(cond, reads, out);
            unused_lets_in_block(then, reads, out);
            unused_lets_in_block(els, reads, out);
        }
        Expr::Throw { value, .. } | Expr::Question { value, .. } => {
            unused_lets_in_expr(value, reads, out);
        }
        Expr::Panic { message, .. } => unused_lets_in_expr(message, reads, out),
        Expr::Match {
            scrutinee, arms, ..
        } => {
            unused_lets_in_expr(scrutinee, reads, out);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    unused_lets_in_expr(guard, reads, out);
                }
                unused_lets_in_expr(&arm.body, reads, out);
            }
        }
        Expr::Try { body, arms, .. } => {
            unused_lets_in_block(body, reads, out);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    unused_lets_in_expr(guard, reads, out);
                }
                unused_lets_in_expr(&arm.body, reads, out);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// effects/over-declared
// ---------------------------------------------------------------------------

/// Compares each root-file function's declared `uses { ... }` row with the
/// row the type checker computed for its body (specs 0023/0035). The merged
/// program's functions are index-parallel with `TypedProgram::functions`;
/// prelude and imported functions are filtered out by their source label.
fn over_declared_effects(
    merged: &ast::Program,
    typed: &TypedProgram,
    label: &str,
    out: &mut Vec<Diagnostic>,
) {
    for (function, typed) in merged.functions.iter().zip(&typed.functions) {
        if function.name_span.file.label != label {
            continue;
        }
        let unused: Vec<&String> = typed
            .effects
            .effects
            .iter()
            .filter(|effect| !typed.body_effects.effects.contains(effect))
            .collect();
        if unused.is_empty() {
            continue;
        }
        let list = unused
            .iter()
            .map(|effect| format!("`{effect}`"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(
            Diagnostic::warning("Over-declared effects")
                .code("effects/over-declared")
                .label(
                    function.name_span.clone(),
                    format!(
                        "`{}` declares {list} in `uses {{ ... }}`, but its body does not use {}",
                        function.name,
                        if unused.len() == 1 { "it" } else { "them" },
                    ),
                )
                .help("Remove the unused effect names from `uses { ... }`."),
        );
    }
}
