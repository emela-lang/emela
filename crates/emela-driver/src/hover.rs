//! Hover information (LSP hover). Looks up the type of the name or expression under the cursor in
//! the type checking results.
//!
//! E (error sets) and R (effect sets) do not normally appear in source, so they are shown here
//! (spec 9.3).

use emela_resolve::hir::{DefId, ExprKind, Item, RefTarget, Res};
use emela_types::{Row, Ty, TyParam};
use line_index::{TextRange, TextSize};

use crate::frontend::TypedProgram;
use crate::pipeline::Analysis;
use crate::source::FileId;

/// `contents` is Markdown. `range` is the position of the hovered name or expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverInfo {
    pub range: TextRange,
    pub contents: String,
}

/// Information at `offset` in `file`.
///
/// - A definition name (fn, const, constructor): `name: type`, the declared type (generalized if
///   inferred). Suspend fns (effect operations and external functions) get `suspend` before the
///   type.
/// - A handler name: `handler H(fields): E` (`E[Int]` with type arguments), plus
///   `init fails E` if init has an E, and `use R` if it has dependencies (shows whether Async
///   remains).
/// - A layer name: `layer L { handlers in initialization order }`, plus `use R` for dependencies
///   unmet inside the layer.
/// - A label name, or a reference to one: its declaration, `effect Debug of Log default`.
/// - A reference to a definition or local: likewise `name: type`.
/// - Any other expression: its type, plus `fails E` if E is not empty and `use R` if R is not
///   empty.
///
/// The innermost (shortest) one is chosen.
pub(crate) fn hover(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    offset: TextSize,
) -> Option<HoverInfo> {
    let mut info = hover_type(program, analysis, file, offset);
    let module = analysis.files.iter().find(|(_, f)| **f == file)?.0;
    let hir = &program.hir;
    let docs = crate::documentation::Documentation::default();
    let target = hir
        .refs
        .iter()
        .find_map(|r| match r.target {
            RefTarget::Def(def) if r.span.module == module && r.span.range.contains(offset) => {
                Some((def, r.span.range))
            }
            _ => None,
        })
        .or_else(|| {
            hir.defs.iter().find_map(|(def, data)| {
                let span = data.span?;
                (span.module == module && span.range.contains(offset)).then_some((def, span.range))
            })
        });
    let mut doc = None;
    if let Some((def, range)) = target {
        // Types and traits do not have value signatures. Supply a declaration when there
        // is no existing type display (for example, on a type annotation).
        let kind = match hir.item(def) {
            Some(Item::Type(_)) => Some("type"),
            Some(Item::Enum(_)) => Some("enum"),
            Some(Item::Trait(_)) => Some("trait"),
            Some(Item::Effect(_)) if label_text(program, def).is_none() => Some("effect"),
            _ => None,
        };
        if info.is_none()
            && let Some(kind) = kind
        {
            info = Some(HoverInfo {
                range,
                contents: code(&format!("{kind} {}", hir.defs[def].name)),
            });
        }
        if matches!(hir.item(def), Some(Item::Fn(_) | Item::TraitFn(_)))
            && program.types.sigs.get(def).is_none()
            && let Some(span) = hir.defs[def].span
            && let Some(text) = crate::documentation::written_signature(analysis, span)
        {
            info = Some(HoverInfo {
                range,
                contents: code(&text),
            });
        }
        doc = docs.definition(program, analysis, def);
    }
    // Handler and trait implementations have name spans but no definition IDs of their own.
    for (def, _) in hir.defs.iter() {
        let span = match hir.item(def) {
            Some(Item::Handler(h)) => h
                .ops
                .iter()
                .map(|op| op.name.span)
                .find(|s| s.module == module && s.range.contains(offset)),
            Some(Item::Impl(i)) => i
                .fns
                .iter()
                .map(|f| f.name.span)
                .find(|s| s.module == module && s.range.contains(offset)),
            _ => None,
        };
        if let Some(span) = span {
            if let Some(text) = crate::documentation::written_signature(analysis, span) {
                info = Some(HoverInfo {
                    range: span.range,
                    contents: code(&text),
                });
            }
            doc = docs.at(analysis, span);
            break;
        }
    }
    // Field declarations have no definition ID.
    for (def, _) in hir.defs.iter() {
        let Some(fields) = hir.item(def).and_then(crate::documentation::fields) else {
            continue;
        };
        if let Some(field) = fields
            .iter()
            .find(|f| f.span.module == module && f.span.range.contains(offset))
        {
            if let Some(span) = hir.types[field.ty].span
                && let Some(&source) = analysis.files.get(span.module)
                && let Some(ty) = analysis.sources[source]
                    .text()
                    .get(usize::from(span.range.start())..usize::from(span.range.end()))
            {
                info = Some(HoverInfo {
                    range: field.span.range,
                    contents: code(&format!("{}: {ty}", field.name)),
                });
            }
            doc = docs.at(analysis, field.span);
            break;
        }
    }
    // Capability operations and record projections are resolved by the type checker.
    if let Some((expr, _, base, name)) = hir
        .exprs
        .iter()
        .filter_map(|(id, e)| {
            let ExprKind::Field { base, name } = &e.kind else {
                return None;
            };
            (name.span.module == module && name.span.range.contains(offset))
                .then_some((id, e, *base, name))
        })
        .min_by_key(|(_, e, _, _)| e.span.range.len())
    {
        if let Some(&op) = program.types.ops.get(expr) {
            doc = docs.definition(program, analysis, op);
        } else if let Some(Ty::Named(con, _)) = program.types.exprs.get(base) {
            let owner = program.types.ty_table.def(*con);
            doc = docs.field(program, analysis, owner, &name.name);
        }
    }
    if info.is_none()
        && let Some((range, text, comment)) =
            crate::documentation::unnamed_doc(analysis, module, offset)
    {
        info = Some(HoverInfo {
            range,
            contents: code(&text),
        });
        doc = Some(comment);
    }
    let mut info = info?;
    if let Some(doc) = doc {
        info.contents.push_str("\n\n---\n\n");
        info.contents.push_str(&doc);
    }
    Some(info)
}

fn hover_type(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    offset: TextSize,
) -> Option<HoverInfo> {
    let module = analysis
        .files
        .iter()
        .find(|(_, f)| **f == file)
        .map(|(m, _)| m)?;
    let hir = &program.hir;
    let types = &program.types;
    let cons = &types.ty_table.cons;
    let contains = |range: TextRange| range.contains(offset);

    // Definition names.
    let def = hir.defs.iter().find(|(_, data)| {
        data.span
            .is_some_and(|s| s.module == module && contains(s.range))
    });
    if let Some((def, data)) = def
        && let Some(sig) = types.sigs.get(def)
    {
        return Some(HoverInfo {
            range: data.span?.range,
            contents: code(&format!(
                "{}: {}{}",
                data.name,
                suspend_prefix(hir, def),
                sig.scheme.display(cons)
            )),
        });
    }
    // Handler names (spec 8.4).
    if let Some((def, data)) = def
        && let Some(text) = handler_text(program, def)
    {
        return Some(HoverInfo {
            range: data.span?.range,
            contents: code(&text),
        });
    }
    // Layer names (spec 8.5).
    if let Some((def, data)) = def
        && let Some(text) = layer_text(program, def)
    {
        return Some(HoverInfo {
            range: data.span?.range,
            contents: code(&text),
        });
    }
    // Label names, and references to labels (`use { Audit }`, `with Audit by H`, issue #255).
    if let Some((def, data)) = def
        && let Some(text) = label_text(program, def)
    {
        return Some(HoverInfo {
            range: data.span?.range,
            contents: code(&text),
        });
    }
    if let Some((range, text)) = hir.refs.iter().find_map(|r| match r.target {
        RefTarget::Def(def) if r.span.module == module && contains(r.span.range) => {
            Some((r.span.range, label_text(program, def)?))
        }
        _ => None,
    }) {
        return Some(HoverInfo {
            range,
            contents: code(&text),
        });
    }

    // Type parameters of the enclosing definition (names of `Bound(i)` in body types).
    let owner = enclosing_def(program, module, offset);
    let params: Vec<TyParam> = owner
        .and_then(|def| types.sigs.get(def))
        .map(|sig| sig.scheme.params.clone())
        .unwrap_or_default();

    // Local binding sites (parameters, binding patterns).
    let local = hir
        .locals
        .iter()
        .filter(|(_, data)| data.span.module == module && contains(data.span.range))
        .min_by_key(|(_, data)| data.span.range.len());
    let expr = hir
        .exprs
        .iter()
        .filter(|(_, e)| e.span.module == module && contains(e.span.range))
        .min_by_key(|(_, e)| e.span.range.len());

    match (local, expr) {
        (Some((local, data)), e)
            if e.is_none_or(|(_, e)| data.span.range.len() <= e.span.range.len()) =>
        {
            let ty = types.locals.get(local)?;
            Some(HoverInfo {
                range: data.span.range,
                contents: code(&format!(
                    "{}: {}",
                    data.name,
                    ty.display_with(cons, &params)
                )),
            })
        }
        (_, Some((expr, e))) => {
            let text = match &e.kind {
                ExprKind::Path(Res::Def(def)) => {
                    let sig = types.sigs.get(*def)?;
                    format!(
                        "{}: {}{}",
                        hir.defs[*def].name,
                        suspend_prefix(hir, *def),
                        sig.scheme.display(cons)
                    )
                }
                ExprKind::Path(Res::Local(local)) => {
                    let ty = types.locals.get(*local)?;
                    format!(
                        "{}: {}",
                        hir.locals[*local].name,
                        ty.display_with(cons, &params)
                    )
                }
                _ => {
                    let ty = types.exprs.get(expr)?;
                    // Suspend operations on capability values (`clock.sleep`).
                    let prefix = types
                        .ops
                        .get(expr)
                        .map_or("", |&op| suspend_prefix(hir, op));
                    let mut text = format!("{prefix}{}", ty.display_with(cons, &params));
                    let errors = types.errors.get(expr).cloned().unwrap_or_else(Row::empty);
                    if !errors.is_empty() {
                        text.push_str(&format!("\nfails {}", errors.display_with(cons, &params)));
                    }
                    let effects = types.effects.get(expr).cloned().unwrap_or_else(Row::empty);
                    if !effects.is_empty() {
                        text.push_str(&format!("\nuse {}", effects.display_effects(cons, &params)));
                    }
                    text
                }
            };
            Some(HoverInfo {
                range: e.span.range,
                contents: code(&text),
            })
        }
        _ => None,
    }
}

/// `handler H(fields): E` (`E[Int]` with type arguments), plus `init fails E` if init
/// has an E (part of the with's E, spec 9.4) and `use R` if it has dependencies (shows whether
/// Async remains, spec 8.7). `None` if `def` is not a handler.
pub(crate) fn handler_text(program: &TypedProgram, def: DefId) -> Option<String> {
    let hir = &program.hir;
    let types = &program.types;
    let cons = &types.ty_table.cons;
    let Some(Item::Handler(item)) = hir.item(def) else {
        return None;
    };
    let mut text = format!("handler {}", hir.defs[def].name);
    let ctor_ty = item
        .ctor
        .and_then(|ctor| types.sigs.get(ctor))
        .map(|sig| &sig.scheme.ty);
    if let (Some(ctor), Some(emela_types::Ty::Fn(f))) = (item.ctor, ctor_ty) {
        let labels = &types.sigs.get(ctor)?.labels;
        let fields: Vec<String> = labels
            .iter()
            .zip(&f.params)
            .map(|(l, ty)| format!("{}: {}", l.as_deref().unwrap_or("_"), ty.display(cons)))
            .collect();
        text.push_str(&format!("({})", fields.join(", ")));
    }
    // An effect with type arguments is shown as `handler H: State[Int]` (spec 8.1).
    match types.sigs.handler_effect(def) {
        Some(tag) => {
            let effect = Row::of_tag(tag.clone());
            text.push_str(&format!(": {}", effect.display_effects(cons, &[])));
        }
        None => {
            if let Res::Def(effect) = item.effect {
                text.push_str(&format!(": {}", hir.defs[effect].name));
            }
        }
    }
    if let Some(fails) = types.handler_fails.get(def)
        && !fails.is_empty()
    {
        text.push_str(&format!("\ninit fails {}", fails.display(cons)));
    }
    if let Some(deps) = types.handler_uses.get(def)
        && !deps.is_empty()
    {
        text.push_str(&format!("\nuse {}", deps.display_effects(cons, &[])));
    }
    Some(text)
}

/// `layer L { handlers in initialization order }`, plus `use R` for dependencies unmet inside
/// the layer (spec 8.5). `None` if `def` is not a layer.
pub(crate) fn layer_text(program: &TypedProgram, def: DefId) -> Option<String> {
    let hir = &program.hir;
    let types = &program.types;
    let layer = types.layers.get(def)?;
    let names: Vec<&str> = layer
        .handlers
        .iter()
        .map(|&h| hir.defs[h].name.as_str())
        .collect();
    let mut text = format!("layer {} {{ {} }}", hir.defs[def].name, names.join(", "));
    if !layer.uses.is_empty() {
        text.push_str(&format!(
            "\nuse {}",
            layer.uses.display_effects(&types.ty_table.cons, &[])
        ));
    }
    Some(text)
}

/// The declaration of a label as written: `effect Debug of Log default` (issue #255). Whether
/// it falls back to its base is read from the declaration; types keep showing the label itself.
/// `None` if `def` is not a label.
pub(crate) fn label_text(program: &TypedProgram, def: DefId) -> Option<String> {
    let hir = &program.hir;
    let Some(Item::Effect(effect)) = hir.item(def) else {
        return None;
    };
    let label = effect.label.as_ref()?;
    // With its type arguments (`State[Int]`).
    let base = match (program.types.sigs.label_base(def), label.base) {
        (Some(tag), _) => Row::of_tag(tag.clone())
            .display_effects(&program.types.ty_table.cons, &[])
            .to_string(),
        (None, Res::Def(base)) => hir.defs[base].name.to_string(),
        (None, _) => "?".to_owned(),
    };
    let default = if label.default { " default" } else { "" };
    Some(format!("effect {} of {base}{default}", hir.defs[def].name))
}

/// The top-level definition (fn or const) whose body contains `offset`.
pub(crate) fn enclosing_def(
    program: &TypedProgram,
    module: emela_resolve::ModuleId,
    offset: TextSize,
) -> Option<DefId> {
    let hir = &program.hir;
    hir.defs.iter().map(|(def, _)| def).find(|&def| {
        let body = match hir.item(def) {
            Some(Item::Fn(item)) => item.body,
            Some(Item::Const(item)) => Some(item.value),
            _ => None,
        };
        body.is_some_and(|body| {
            let span = hir.exprs[body].span;
            span.module == module && span.range.contains(offset)
        })
    })
}

/// The marker placed before the type of a suspend fn (effect operations and
/// `@external suspend fn`).
fn suspend_prefix(hir: &emela_resolve::hir::Program, def: DefId) -> &'static str {
    let suspend = match hir.item(def) {
        Some(Item::Op(op)) => op.sig.is_suspend,
        Some(Item::Fn(f)) => f.sig.is_suspend,
        _ => false,
    };
    if suspend { "suspend " } else { "" }
}

/// A block of Emela code.
fn code(text: &str) -> String {
    format!("```emela\n{text}\n```")
}
