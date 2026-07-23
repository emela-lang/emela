//! textDocument/codeAction (spec 0033): quickfixes recomputed from the last
//! snapshot at the requested range. The server re-checks synchronously on
//! every change, so the snapshot is at least as fresh as any diagnostic the
//! client could echo back — `context.diagnostics` is ignored entirely.

use std::collections::{HashMap, HashSet};

use crate::ast::{Block, BlockItem, Expr, MatchArm, TraitMethodSig, Type};
use crate::error::Span;
use crate::lexer::{TokenKind, lex};
use crate::lsp::analysis::{EnumSym, Snapshot, VariantSym, render_type};
use crate::lsp::completion::scrutinee_type;
use crate::lsp::documents::Document;
use crate::lsp::position::{offset_to_position, position_to_offset};
use crate::lsp::protocol::{CodeAction, Range, TextEdit, WorkspaceEdit};
use crate::typecheck::{EntryKind, subst_type};

pub(crate) fn actions(doc: &Document, range: &Range, snapshot: &Snapshot) -> Vec<CodeAction> {
    let start = position_to_offset(&doc.text, &range.start);
    let end = position_to_offset(&doc.text, &range.end);
    let mut actions = Vec::new();
    if let Some(action) = fill_match_arms(doc, start, end, snapshot) {
        actions.push(action);
    }
    if let Some(action) = implement_missing_methods(doc, start, end, snapshot) {
        actions.push(action);
    }
    actions
}

/// The quickfix for a non-exhaustive `match` (spec 0005): appends one arm per
/// missing variant, `Enum::Variant(_) -> panic("TODO: handle Variant")`.
fn fill_match_arms(
    doc: &Document,
    start: usize,
    end: usize,
    snapshot: &Snapshot,
) -> Option<CodeAction> {
    let (scrutinee, arms, match_span) = innermost_match(snapshot, start, end)?;
    // Replicate `check_exhaustive` exactly: a guarded arm never counts toward
    // coverage, a wildcard or binding pattern covers everything.
    let mut covered: HashSet<&str> = HashSet::new();
    for arm in arms {
        if arm.guard.is_some() {
            continue;
        }
        match &arm.pattern {
            crate::ast::Pattern::Wildcard(_) | crate::ast::Pattern::Binding { .. } => return None,
            crate::ast::Pattern::Variant { variant, .. } => {
                covered.insert(variant.as_str());
            }
        }
    }
    let enum_sym = scrutinee_enum(scrutinee, snapshot)?;
    let missing: Vec<&VariantSym> = enum_sym
        .variants
        .iter()
        .filter(|variant| !covered.contains(variant.name.as_str()))
        .collect();
    if missing.is_empty() {
        return None;
    }
    let (insert_at, indent) = match arms.last() {
        Some(last) => (last.span.end, line_indent(&doc.text, last.span.start)),
        None => (
            open_brace_end(&doc.text, &scrutinee.span(), match_span)?,
            format!("{}  ", line_indent(&doc.text, match_span.start)),
        ),
    };
    let mut new_text = String::new();
    for variant in &missing {
        new_text.push('\n');
        new_text.push_str(&indent);
        new_text.push_str(&arm_text(&enum_sym.name, variant));
    }
    let position = offset_to_position(&doc.text, insert_at);
    let title = format!(
        "Add missing match arms ({})",
        missing
            .iter()
            .map(|variant| variant.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    Some(CodeAction {
        title,
        kind: "quickfix",
        edit: WorkspaceEdit {
            changes: HashMap::from([(
                doc.uri.clone(),
                vec![TextEdit {
                    range: Range {
                        start: position,
                        end: position,
                    },
                    new_text,
                }],
            )]),
        },
    })
}

/// The quickfix for an incomplete impl (spec 0020): appends a stub for every
/// trait method without a default that the impl does not define. The compiler
/// reports only the first missing method; the action fills them all.
fn implement_missing_methods(
    doc: &Document,
    start: usize,
    end: usize,
    snapshot: &Snapshot,
) -> Option<CodeAction> {
    let decl = snapshot
        .entry_impls
        .iter()
        .filter(|decl| decl.span.start <= start && end <= decl.span.end)
        .min_by_key(|decl| decl.span.end - decl.span.start)?;
    let trait_decl = snapshot
        .trait_decls
        .iter()
        .find(|t| t.name == decl.trait_name)?;
    let missing: Vec<&TraitMethodSig> = trait_decl
        .methods
        .iter()
        .filter(|method| {
            method.default_body.is_none()
                && !decl
                    .methods
                    .iter()
                    .any(|written| written.name == method.name)
        })
        .collect();
    if missing.is_empty() {
        return None;
    }
    let mut subst = HashMap::new();
    subst.insert("Self".to_string(), decl.target.clone());
    // Insert after the last written method's body, or right after the block's
    // opening brace when the impl is empty.
    let (insert_at, indent) = match decl.methods.last() {
        Some(last) => (
            last.body.span.end,
            line_indent(&doc.text, last.name_span.start),
        ),
        None => (
            open_brace_end(&doc.text, &decl.target_span, &decl.span)?,
            format!("{}  ", line_indent(&doc.text, decl.span.start)),
        ),
    };
    let mut new_text = String::new();
    for method in &missing {
        new_text.push('\n');
        new_text.push_str(&indent);
        new_text.push_str(&stub_text(method, &subst, &indent));
    }
    let position = offset_to_position(&doc.text, insert_at);
    let title = format!(
        "Implement missing methods ({})",
        missing
            .iter()
            .map(|method| method.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    Some(CodeAction {
        title,
        kind: "quickfix",
        edit: WorkspaceEdit {
            changes: HashMap::from([(
                doc.uri.clone(),
                vec![TextEdit {
                    range: Range {
                        start: position,
                        end: position,
                    },
                    new_text,
                }],
            )]),
        },
    })
}

/// One method stub with `Self` substituted to the impl target. The signature
/// reproduces the trait's `throws` and `uses` rows exactly — `check_impl_sig`
/// demands strict equality — and the body is a `panic`, which types as `Never`
/// and so fits any return type with no effects and no throws (spec 0011).
fn stub_text(method: &TraitMethodSig, subst: &HashMap<String, Type>, indent: &str) -> String {
    let params: Vec<String> = method
        .params
        .iter()
        .map(|param| {
            format!(
                "{}: {}",
                param.name,
                render_type(&subst_type(&param.ty, subst))
            )
        })
        .collect();
    let mut sig = format!(
        "fn {}({}) -> {}",
        method.name,
        params.join(", "),
        render_type(&subst_type(&method.ret, subst))
    );
    if let Some(throws) = &method.throws {
        sig.push_str(&format!(
            " throws {}",
            render_type(&subst_type(throws, subst))
        ));
    }
    if !method.effects.effects.is_empty() {
        sig.push_str(&format!(
            " uses {{ {} }}",
            method.effects.effects.join(", ")
        ));
    }
    format!(
        "{sig} {{\n{indent}  panic(\"TODO: implement {}\")\n{indent}}}",
        method.name
    )
}

/// The scrutinee's enum, resolved through the type index — recorded even when
/// the match itself failed exhaustiveness, because the scrutinee is checked
/// first. Falls back to the annotation heuristic completion uses when an
/// earlier error stopped checking before this match. A record scrutinee also
/// arrives as `Type::Enum` but is absent from `Snapshot::enums`, so it
/// correctly yields no action.
fn scrutinee_enum<'a>(scrutinee: &Expr, snapshot: &'a Snapshot) -> Option<&'a EnumSym> {
    let span = scrutinee.span();
    let ty = snapshot
        .type_index
        .iter()
        .find(|entry| {
            entry.span.start == span.start
                && entry.span.end == span.end
                && matches!(entry.kind, EntryKind::Expr)
        })
        .map(|entry| entry.ty.clone())
        .or_else(|| {
            if let Expr::Var(name, _) = scrutinee {
                scrutinee_type(name, span.start, snapshot)
            } else {
                None
            }
        })?;
    let Type::Enum(name, _) = ty else {
        return None;
    };
    snapshot.enums.iter().find(|sym| sym.name == name)
}

/// The innermost `match` whose span contains the requested range.
fn innermost_match(
    snapshot: &Snapshot,
    start: usize,
    end: usize,
) -> Option<(&Expr, &[MatchArm], &Span)> {
    let mut matches = Vec::new();
    for function in &snapshot.entry_functions {
        collect_matches_block(&function.body, &mut matches);
    }
    matches
        .into_iter()
        .filter_map(|expr| match expr {
            Expr::Match {
                scrutinee,
                arms,
                span,
            } if span.start <= start && end <= span.end => {
                Some((scrutinee.as_ref(), arms.as_slice(), span))
            }
            _ => None,
        })
        .min_by_key(|(_, _, span)| span.end - span.start)
}

/// Collects every `match` expression, at any nesting depth.
fn collect_matches<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
    match expr {
        Expr::Match {
            scrutinee, arms, ..
        } => {
            out.push(expr);
            collect_matches(scrutinee, out);
            collect_matches_arms(arms, out);
        }
        Expr::Try { body, arms, .. } => {
            collect_matches_block(body, out);
            collect_matches_arms(arms, out);
        }
        Expr::Array(elements, _) => {
            for element in elements {
                collect_matches(element, out);
            }
        }
        Expr::Call { callee, args, .. } => {
            collect_matches(callee, out);
            for arg in args {
                collect_matches(arg, out);
            }
        }
        Expr::Fn { body, .. } => collect_matches_block(body, out),
        Expr::Binary { left, right, .. } => {
            collect_matches(left, out);
            collect_matches(right, out);
        }
        Expr::Block(block) => collect_matches_block(block, out),
        Expr::If {
            cond, then, els, ..
        } => {
            collect_matches(cond, out);
            collect_matches_block(then, out);
            collect_matches_block(els, out);
        }
        Expr::Throw { value, .. } | Expr::Question { value, .. } => collect_matches(value, out),
        Expr::Panic { message, .. } => collect_matches(message, out),
        Expr::RecordLiteral { fields, .. } => {
            for (_, _, value) in fields {
                collect_matches(value, out);
            }
        }
        Expr::Field { target, .. } => collect_matches(target, out),
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Bool(..)
        | Expr::String(..)
        | Expr::Char(..)
        | Expr::Unit(..)
        | Expr::Var(..)
        | Expr::Path { .. }
        | Expr::TypePath { .. } => {}
    }
}

fn collect_matches_arms<'a>(arms: &'a [MatchArm], out: &mut Vec<&'a Expr>) {
    for arm in arms {
        if let Some(guard) = &arm.guard {
            collect_matches(guard, out);
        }
        collect_matches(&arm.body, out);
    }
}

fn collect_matches_block<'a>(block: &'a Block, out: &mut Vec<&'a Expr>) {
    for item in &block.items {
        match item {
            BlockItem::Let { value, .. } => collect_matches(value, out),
            BlockItem::Expr(expr) => collect_matches(expr, out),
        }
    }
}

/// The leading whitespace of the line containing `offset`.
fn line_indent(text: &str, offset: usize) -> String {
    let line_start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
    text[line_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

/// The end offset of a block's opening `{`: the first `{` token after `after`
/// and within `within` — a match's arm list after its scrutinee, an impl's
/// body after its target type. Found by lexing, so a `{` inside a string
/// literal never matches.
fn open_brace_end(text: &str, after: &Span, within: &Span) -> Option<usize> {
    let (tokens, _) = lex("<code-action>", text);
    tokens
        .iter()
        .find(|token| {
            token.kind == TokenKind::LBrace
                && token.span.start >= after.end
                && token.span.start < within.end
        })
        .map(|token| token.span.end)
}

/// One generated arm: payload fields are ignored with `_` so the arm compiles
/// as-is, and `panic` types as `Never`, fitting any arm type (spec 0011).
fn arm_text(enum_name: &str, variant: &VariantSym) -> String {
    let pattern = if variant.arity == 0 {
        format!("{enum_name}::{}", variant.name)
    } else {
        format!(
            "{enum_name}::{}({})",
            variant.name,
            vec!["_"; variant.arity].join(", ")
        )
    };
    format!("{pattern} -> panic(\"TODO: handle {}\")", variant.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_indent_of_offset() {
        let text = "match c {\n    Red -> 1\n}";
        let red = text.find("Red").unwrap();
        assert_eq!(line_indent(text, red), "    ");
        assert_eq!(line_indent(text, 0), "");
    }

    #[test]
    fn arm_text_by_arity() {
        let unit = VariantSym {
            name: "Red".to_string(),
            arity: 0,
        };
        let pair = VariantSym {
            name: "Rgb".to_string(),
            arity: 3,
        };
        assert_eq!(
            arm_text("Color", &unit),
            "Color::Red -> panic(\"TODO: handle Red\")"
        );
        assert_eq!(
            arm_text("Color", &pair),
            "Color::Rgb(_, _, _) -> panic(\"TODO: handle Rgb\")"
        );
    }

    #[test]
    fn stub_text_substitutes_self_and_reproduces_rows() {
        use crate::ast::{EffectRow, Param};
        use crate::error::SourceFile;
        let source = SourceFile::new("test.emel", "");
        let span = Span::new(source, 0, 0);
        let mut effects = EffectRow::default();
        effects.effects.push("Io".to_string());
        let method = TraitMethodSig {
            name: "greet".to_string(),
            name_span: span.clone(),
            params: vec![Param {
                name: "x".to_string(),
                name_span: span,
                ty: Type::Var("Self".to_string()),
            }],
            ret: Type::String,
            throws: Some(Type::Enum("E".to_string(), Vec::new())),
            effects,
            default_body: None,
        };
        let subst = HashMap::from([(
            "Self".to_string(),
            Type::Enum("Foo".to_string(), Vec::new()),
        )]);
        assert_eq!(
            stub_text(&method, &subst, "  "),
            "fn greet(x: Foo) -> String throws E uses { Io } {\n    panic(\"TODO: implement greet\")\n  }"
        );
    }

    #[test]
    fn open_brace_end_skips_string_braces() {
        let text = "match f(\"{\") {\n}";
        let scrutinee_end = text.find(") {").unwrap() + 1;
        let source = crate::error::SourceFile::new("test.emel", text);
        let scrutinee = Span::new(source.clone(), 6, scrutinee_end);
        let whole = Span::new(source, 0, text.len());
        assert_eq!(open_brace_end(text, &scrutinee, &whole), Some(14));
    }
}
