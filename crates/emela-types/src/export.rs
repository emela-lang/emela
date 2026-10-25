//! Checks for `@export` and `@entry` (spec 12.7, 12.8).
//!
//! - The parameters and the return value of an `@export` fn are boundary types (E0436). The first
//!   parameter of an entry adapter is not checked: it is the function the adapter receives.
//! - An `@export` fn that is not an entry adapter is allowed only in the entry module (E0437).
//! - Two exports with the same host name for the same target are an error (E0438). The middle
//!   argument of the three-argument form is part of the host name (`@export(js, "add")` and
//!   `@export(js, "default", "add")` are different).
//! - `@entry(T)` names an entry adapter (E0439) and the fn must fit its first parameter (E0440).
//!
//! The R of an `@export` fn is checked with the other closed contexts (E0402, spec 12.7): the
//! entry module's fns with `@export` are registered in `check_program_with`.

use emela_resolve::hir::{DefId, DefModule, Item, Program, Res, Span};
use smol_str::SmolStr;

use crate::diagnostic::{BoundaryPosition, Diagnostic, DiagnosticKind};
use crate::infer::InferCtx;
use crate::lower::TyTable;
use crate::row::RowElem;
use crate::signature::{Signature, Signatures};
use crate::ty::{Prim, Ty, TyConKind};

/// One host name of an export: the target, the middle argument of the three-argument form, and
/// the name.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ExportKey {
    target: SmolStr,
    interface: Option<SmolStr>,
    name: SmolStr,
}

pub(crate) fn check(
    program: &Program,
    options: &crate::check::CheckOptions,
    sigs: &Signatures,
    ty_table: &TyTable,
    diags: &mut Vec<Diagnostic>,
) {
    let mut exports: Vec<(ExportKey, Span)> = Vec::new();
    let mut start_needed = false;
    for (def, data) in program.defs.iter() {
        let Some(Item::Fn(item)) = program.item(def) else {
            continue;
        };
        let Some(sig) = sigs.get(def) else {
            continue;
        };
        let adapter = is_entry_adapter(sig);
        if !item.exports.is_empty() {
            // The E of an `@export` fn MUST be empty, entry adapter or not (E0402, spec 12.7).
            diags.extend(fails_errors(program, def, sig, ty_table));
            let boundary = if adapter {
                boundary_errors(program, def, item, sig, ty_table, 1)
            } else {
                if let Some(entry) = options.entry
                    && data.module != DefModule::Source(entry)
                {
                    diags.push(Diagnostic {
                        span: Some(item.exports[0].span),
                        kind: DiagnosticKind::ExportOutsideEntryModule,
                    });
                }
                for export in &item.exports {
                    if let Some(key) = key_of(export) {
                        if key.target == "js" {
                            start_needed |= may_suspend(program, sig);
                        }
                        exports.push((key, export.span));
                    }
                }
                boundary_errors(program, def, item, sig, ty_table, 0)
            };
            diags.extend(boundary);
        }
        // Only the first `@entry` is used; the rest are an error (spec 12.8 has one entry per fn
        // and no code for the duplicate, so it is reported without one).
        for extra in item.entries.iter().skip(1) {
            diags.push(Diagnostic {
                span: Some(extra.span),
                kind: DiagnosticKind::MultipleEntryAnnotations,
            });
        }
        for entry in &item.entries {
            if let Some(entry_module) = options.entry
                && data.module != DefModule::Source(entry_module)
            {
                diags.push(Diagnostic {
                    span: Some(entry.span),
                    kind: DiagnosticKind::EntryOutsideEntryModule,
                });
            }
        }
        if let Some(entry) = item.entries.first()
            && let Res::Def(adapter_def) = entry.adapter
        {
            let Some(adapter_sig) = sigs.get(adapter_def) else {
                continue;
            };
            let is_adapter = matches!(program.item(adapter_def), Some(Item::Fn(a)) if !a.exports.is_empty())
                && is_entry_adapter(adapter_sig);
            if !is_adapter {
                diags.push(Diagnostic {
                    span: Some(entry.span),
                    kind: DiagnosticKind::NotEntryAdapter {
                        path: entry.path.clone(),
                    },
                });
                continue;
            }
            if !fits(sigs, adapter_def, def) {
                diags.push(Diagnostic {
                    span: Some(entry.span),
                    kind: DiagnosticKind::EntryDoesNotFit {
                        function: data.name.clone(),
                        adapter: entry.path.clone(),
                    },
                });
            }
            if let Some(Item::Fn(adapter_item)) = program.item(adapter_def) {
                for export in &adapter_item.exports {
                    if let Some(key) = key_of(export) {
                        if key.target == "js" {
                            start_needed |= may_suspend(program, adapter_sig);
                            start_needed |= may_suspend(program, sig);
                        }
                        exports.push((key, entry.span));
                    }
                }
            }
        }
    }

    // In source order, so that the second occurrence is reported. The launcher of `main.mjs`
    // exports `main` only when the entry has it, uses `$start` when `main`, a selected test or an
    // exported fn may suspend, and keeps the default export separate from a named export called
    // `default` (spec 16.6.2). `CheckOptions.tests` is every test, so a build without a suspending
    // test still reserves `$start` here; `emela test` may select one of them.
    if let Some(main) = options.main
        && let Some(sig) = sigs.get(main)
    {
        start_needed |= may_suspend(program, sig);
    }
    for &test in &options.tests {
        if let Some(sig) = sigs.get(test) {
            start_needed |= may_suspend(program, sig);
        }
    }
    exports.sort_by_key(|(_, span)| (span.module.into_raw().into_u32(), span.range.start()));
    let mut reserved = vec!["default"];
    if options.main.is_some() {
        reserved.push("main");
    }
    if start_needed {
        reserved.push("$start");
    }
    let mut seen: Vec<ExportKey> = reserved
        .into_iter()
        .map(|name| ExportKey {
            target: "js".into(),
            interface: None,
            name: name.into(),
        })
        .collect();
    for (key, span) in exports {
        if seen.contains(&key) {
            diags.push(Diagnostic {
                span: Some(span),
                kind: DiagnosticKind::DuplicateExport {
                    name: key.name,
                    target: key.target,
                    interface: key.interface,
                },
            });
        } else {
            seen.push(key);
        }
    }
}

fn key_of(export: &emela_resolve::hir::ExportAttr) -> Option<ExportKey> {
    match export.args.as_slice() {
        [name] => Some(ExportKey {
            target: export.target.clone(),
            interface: None,
            name: name.as_str().into(),
        }),
        [interface, name] => Some(ExportKey {
            target: export.target.clone(),
            interface: Some(interface.as_str().into()),
            name: name.as_str().into(),
        }),
        // The syntax check reported the shape (E0146).
        _ => None,
    }
}

/// E0402 for a non-empty E on an `@export` fn. The declared E is what the fn may fail with, so a
/// written `fails` clause is enough to reject it even when the body never fails.
fn fails_errors(
    program: &Program,
    def: DefId,
    sig: &Signature,
    ty_table: &TyTable,
) -> Vec<Diagnostic> {
    let Some(f) = fn_ty(sig) else {
        return Vec::new();
    };
    if f.fails.is_empty() {
        return Vec::new();
    }
    let errors: Vec<SmolStr> = f
        .fails
        .elems()
        .iter()
        .map(|e| match e {
            RowElem::Tag(t) => program.defs[t.def].name.clone(),
            RowElem::Of(ty) => SmolStr::new(ty.display(&ty_table.cons).to_string()),
            RowElem::Bound(i) => sig
                .scheme
                .params
                .get(*i as usize)
                .map_or_else(|| SmolStr::new("?"), |p| p.name.clone()),
            RowElem::Var(v) => SmolStr::new(format!("?e{}", v.index())),
        })
        .collect();
    vec![Diagnostic {
        span: program.defs[def].span,
        kind: DiagnosticKind::ExportFails {
            function: program.defs[def].name.clone(),
            errors,
        },
    }]
}

/// Whether the first parameter of `sig` has a function type (spec 12.8).
fn is_entry_adapter(sig: &Signature) -> bool {
    match &sig.scheme.ty {
        Ty::Fn(f) => matches!(f.params.first(), Some(Ty::Fn(_))),
        _ => false,
    }
}

/// The fn type of a signature, if it has one.
fn fn_ty(sig: &Signature) -> Option<&crate::ty::FnTy> {
    match &sig.scheme.ty {
        Ty::Fn(f) => Some(f),
        _ => None,
    }
}

/// Whether the R of a signature contains `Async`, the only effect an `@export` fn may have
/// (spec 12.7). An open set counts as maybe suspending, as JavaScript then needs `$start`.
fn may_suspend(program: &Program, sig: &Signature) -> bool {
    let Some(async_) = program.prelude_def("Async") else {
        return true;
    };
    match fn_ty(sig) {
        Some(f) => {
            f.uses
                .elems()
                .iter()
                .any(|e| matches!(e, RowElem::Tag(t) if t.def == async_))
                || f.uses
                    .elems()
                    .iter()
                    .any(|e| matches!(e, RowElem::Bound(_) | RowElem::Var(_)))
        }
        None => false,
    }
}

/// Whether `f` is accepted as the first argument of `adapter` (E0440). Both signatures are
/// instantiated, so a generic entry adapter (`Http.serve[E]`) accepts a callback at the caller's
/// types.
fn fits(sigs: &Signatures, adapter: DefId, f: DefId) -> bool {
    let (Some(adapter_sig), Some(f_sig)) = (sigs.get(adapter), sigs.get(f)) else {
        return false;
    };
    let mut ctx = InferCtx::new();
    let adapter_ty = ctx.instantiate(&adapter_sig.scheme);
    let Ty::Fn(adapter_fn) = adapter_ty else {
        return false;
    };
    let Some(first) = adapter_fn.params.first() else {
        return false;
    };
    let f_ty = ctx.instantiate(&f_sig.scheme);
    ctx.unify(first, &f_ty).is_ok() && ctx.subsets_hold()
}

/// E0436 for the parameters of `sig` from `skip` and its return value. `skip` is 1 for an entry
/// adapter, whose first parameter is the fn it receives.
fn boundary_errors(
    program: &Program,
    def: DefId,
    item: &emela_resolve::hir::FnItem,
    sig: &Signature,
    ty_table: &TyTable,
    skip: usize,
) -> Vec<Diagnostic> {
    let Some(f) = fn_ty(sig) else {
        return Vec::new();
    };
    let function: SmolStr = program.defs[def].name.clone();
    let mut diags = Vec::new();
    let mut push = |ty: &Ty, position, span: Option<Span>| {
        if boundary(ty, program, ty_table) {
            return;
        }
        diags.push(Diagnostic {
            span: span.or(program.defs[def].span),
            kind: DiagnosticKind::NotBoundaryType {
                function: function.clone(),
                ty: SmolStr::new(
                    ty.display_with(&ty_table.cons, &sig.scheme.params)
                        .to_string(),
                ),
                position,
            },
        });
    };
    for (i, ty) in f.params.iter().enumerate().skip(skip) {
        let span = item
            .sig
            .params
            .get(i)
            .and_then(|p| p.ty)
            .and_then(|id| program.types[id].span);
        push(ty, BoundaryPosition::Parameter, span);
    }
    let ret_span = item
        .sig
        .ret
        .and_then(|id| program.types[id].span)
        .or(program.defs[def].span);
    push(&f.ret, BoundaryPosition::Return, ret_span);
    diags
}

/// Whether `ty` is a boundary type (spec 12.4): a primitive, `()`, an `@external type`, or a
/// function type made only of those.
fn boundary(ty: &Ty, program: &Program, ty_table: &TyTable) -> bool {
    match ty {
        Ty::Prim(
            Prim::Int | Prim::Int64 | Prim::Float | Prim::Bool | Prim::String | Prim::Unit,
        ) => true,
        Ty::Named(id, args) if args.is_empty() => external_type(program, ty_table, *id),
        Ty::Fn(f) => {
            f.params.iter().all(|p| boundary(p, program, ty_table))
                && boundary(&f.ret, program, ty_table)
        }
        _ => false,
    }
}

/// An `@external type`: a `type` with no fields (spec 12.3). A type without fields must be
/// `@external`, so the shape decides it.
fn external_type(program: &Program, ty_table: &TyTable, id: crate::ty::TyConId) -> bool {
    if ty_table.cons[id].kind != TyConKind::Type {
        return false;
    }
    matches!(
        program.item(ty_table.def_of(id)),
        Some(Item::Type(t)) if matches!(t.fields, emela_resolve::hir::Fields::None)
    )
}
