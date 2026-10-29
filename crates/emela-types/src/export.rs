//! Exported functions and entry adapters (`[ext.export]`, `[ext.entry]`).
//!
//! [`add_entry_exports`] runs before type checking, because it adds definitions: for each
//! `@entry(T)` on `f`, a fn whose body is `T(f, ...)`. Type checking then infers it like any other
//! fn, so matching `f` against the first parameter of `T` (and instantiating `T`) needs nothing of
//! its own; an error in that body is reported as E0440 instead ([`check`]).
//!
//! [`check`] runs after inference: the boundary types of an exported fn (E0436) and its empty E
//! (E0402). Its R is closed like main's ([`crate::CheckOptions`] is not involved: every fn with an
//! `@export` is closed, see `check_program_with`).

use emela_resolve::ModuleId;
use emela_resolve::hir::{
    Arg, Block, DefData, DefId, DefKind, DefModule, EntryExport, Expr, ExprKind, ExternalAttr,
    FnItem, FnSig, Item, LocalData, LocalKind, Param, Program, Res, Span, TypeId, TypeKind,
};
use rowan::TextRange;
use smol_str::SmolStr;

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::lower::TyTable;
use crate::signature::Signatures;
use crate::ty::{Prim, Ty};

/// Whether `def` is an entry adapter: an `@export` fn whose first parameter has a function type
/// (`[ext.entry]`).
pub fn is_entry_adapter(program: &Program, def: DefId) -> bool {
    let Some(Item::Fn(f)) = program.item(def) else {
        return false;
    };
    !f.exports.is_empty()
        && f.sig
            .params
            .first()
            .and_then(|p| p.ty)
            .is_some_and(|ty| matches!(program.types[ty].kind, TypeKind::Fn { .. }))
}

/// Whether the program is an HTTP service (`[lib.http-service]`): an fn of the entry module has
/// `@entry(Http.serve)`, the entry adapter of the bundled `Std.Http`. `Std` cannot be the root
/// name of a project or a dependency (spec 4.1), so the module name identifies the bundled one.
pub fn is_http_service(program: &Program) -> bool {
    program
        .entry_exports
        .iter()
        .any(|e| is_http_serve(program, e.adapter))
}

/// Whether `def` is `serve` of the bundled `Std.Http`.
pub fn is_http_serve(program: &Program, def: DefId) -> bool {
    let data = &program.defs[def];
    data.name == "serve"
        && matches!(data.module, DefModule::Source(m)
            if program.modules.get(m).is_some_and(|items| items.name.segments() == ["Std", "Http"]))
}

/// Checks where `@export` and `@entry` are written (E0437, E0439) and that no two exports share a
/// name (E0438), and adds the fns that `@entry` exposes ([`Program::entry_exports`]). `entry` is
/// the entry module; without one, every `@export` that is not an entry adapter and every `@entry`
/// is outside it.
pub fn add_entry_exports(program: &mut Program, entry: Option<ModuleId>) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let mut entries = Vec::new();
    let fns: Vec<DefId> = program
        .items
        .iter()
        .filter(|(_, item)| matches!(item, Item::Fn(_)))
        .map(|(def, _)| def)
        .collect();
    for def in fns {
        let Some(Item::Fn(f)) = program.item(def) else {
            continue;
        };
        let in_entry = entry.is_some_and(|m| program.defs[def].module == DefModule::Source(m));
        if !in_entry && !f.exports.is_empty() && !is_entry_adapter(program, def) {
            diags.push(Diagnostic {
                span: Some(f.exports[0].span),
                kind: DiagnosticKind::ExportOutsideEntry { attr: "export" },
            });
        }
        for attr in &f.entries {
            if !in_entry {
                diags.push(Diagnostic {
                    span: Some(attr.span),
                    kind: DiagnosticKind::ExportOutsideEntry { attr: "entry" },
                });
                continue;
            }
            match attr.adapter {
                // Already reported by name resolution.
                Some(Res::Err) => {}
                Some(Res::Def(adapter)) if is_entry_adapter(program, adapter) => {
                    entries.push((def, adapter, attr.span));
                }
                Some(Res::Def(adapter)) => diags.push(Diagnostic {
                    span: Some(attr.span),
                    kind: DiagnosticKind::NotEntryAdapter {
                        name: Some(display_name(program, adapter, entry)),
                    },
                }),
                Some(Res::Local(_)) | None => diags.push(Diagnostic {
                    span: Some(attr.span),
                    kind: DiagnosticKind::NotEntryAdapter { name: None },
                }),
            }
        }
    }
    for (f, adapter, span) in entries {
        if let Some(def) = entry_fn(program, f, adapter, span) {
            program.entry_exports.push(EntryExport {
                def,
                entry: f,
                adapter,
                span,
            });
        }
    }
    duplicates(program, entry, &mut diags);
    diags
}

/// `Http.serve` for a fn of another module, `serve` for one of the entry module.
fn display_name(program: &Program, def: DefId, entry: Option<ModuleId>) -> SmolStr {
    let data = &program.defs[def];
    match data.module {
        DefModule::Source(m) if Some(m) != entry => match program.modules.get(m) {
            Some(items) if !items.name.segments().is_empty() => {
                SmolStr::new(format!("{}.{}", items.name.last(), data.name))
            }
            _ => data.name.clone(),
        },
        _ => data.name.clone(),
    }
}

/// The span of the expressions and parameters of a fn made by `@entry` at `attr`: an empty range at
/// the end of the attribute. Being empty, it covers no position, so hover, references and inlay
/// hints do not see the fn; being unique to the attribute, it tells which diagnostics are about the
/// fn ([`check`]).
fn body_span(attr: Span) -> Span {
    Span {
        module: attr.module,
        range: TextRange::empty(attr.range.end()),
    }
}

/// Adds `fn f$entry(p1: T1, ...) -> R { adapter(f, p1, ...) }` to the entry module, with the
/// parameters and return type of `adapter` after its first parameter. It has no span of its own,
/// and its expressions and parameters have [`body_span`].
///
/// `None` if a parameter or the return type of `adapter` names one of its type parameters (which
/// is not a boundary type, E0436 on `adapter`): the new fn could not refer to it.
fn entry_fn(program: &mut Program, f: DefId, adapter: DefId, attr: Span) -> Option<DefId> {
    let span = body_span(attr);
    let Some(Item::Fn(item)) = program.item(adapter) else {
        return None;
    };
    let rest: Vec<(SmolStr, Option<TypeId>)> = item.sig.params[1..]
        .iter()
        .map(|p| (program.locals[p.local].name.clone(), p.ty))
        .collect();
    let ret = item.sig.ret;
    let own_params = &item.sig.type_params;
    let mentions_param =
        |ty: TypeId| mentions(program, ty, &|def| own_params.iter().any(|p| p.def == def));
    if rest.iter().filter_map(|(_, ty)| *ty).any(mentions_param) || ret.is_some_and(mentions_param)
    {
        return None;
    }
    let count = program
        .entry_exports
        .iter()
        .filter(|e| e.entry == f)
        .count();
    let name = SmolStr::new(format!("{}$entry{count}", program.defs[f].name));
    let def = program.defs.alloc(DefData {
        name,
        kind: DefKind::Fn,
        module: program.defs[f].module.clone(),
        parent: None,
        span: None,
        is_pub: false,
        is_opaque: false,
    });
    let expr = |program: &mut Program, kind| program.exprs.alloc(Expr { kind, span });
    let mut params = Vec::new();
    let mut args = vec![Arg {
        label: None,
        value: expr(program, ExprKind::Path(Res::Def(f))),
        punned: false,
    }];
    for (name, ty) in rest {
        let local = program.locals.alloc(LocalData {
            name,
            kind: LocalKind::Param,
            span,
        });
        params.push(Param { local, ty });
        args.push(Arg {
            label: None,
            value: expr(program, ExprKind::Path(Res::Local(local))),
            punned: false,
        });
    }
    let callee = expr(program, ExprKind::Path(Res::Def(adapter)));
    let call = expr(program, ExprKind::Call { callee, args });
    let body = expr(
        program,
        ExprKind::Block(Block {
            stmts: Vec::new(),
            tail: Some(call),
        }),
    );
    program.items.insert(
        def,
        Item::Fn(FnItem {
            sig: FnSig {
                type_params: Vec::new(),
                params,
                ret,
                fails: None,
                uses: None,
                is_suspend: false,
            },
            body: Some(body),
            is_external: false,
            externals: Vec::new(),
            is_intrinsic: false,
            exports: Vec::new(),
            entries: Vec::new(),
        }),
    );
    Some(def)
}

/// Whether the type `ty` names a definition for which `is` holds.
fn mentions(program: &Program, ty: TypeId, is: &dyn Fn(DefId) -> bool) -> bool {
    match &program.types[ty].kind {
        TypeKind::Path { res, args } => {
            matches!(res, Res::Def(def) if is(*def))
                || args.iter().any(|&a| mentions(program, a, is))
        }
        TypeKind::Tuple(elems) => elems.iter().any(|&a| mentions(program, a, is)),
        TypeKind::Fn { params, ret, .. } => {
            params.iter().any(|&a| mentions(program, a, is))
                || ret.is_some_and(|r| mentions(program, r, is))
        }
        _ => false,
    }
}

/// A fn that the program exposes to the host, with its host names (`[ext.export]`).
#[derive(Debug, Clone, Copy)]
pub struct HostExports<'p> {
    pub def: DefId,
    /// The `@export`s of the fn, or of the entry adapter for a fn made by `@entry`.
    pub exports: &'p [ExternalAttr],
    /// The `@entry` attribute, for a fn made by `@entry`.
    pub entry: Option<Span>,
}

/// The fns that the program exposes: the entry module's `@export` fns that are not entry adapters,
/// then the fns made by `@entry`.
pub fn host_exports(program: &Program, entry: Option<ModuleId>) -> Vec<HostExports<'_>> {
    let mut out = Vec::new();
    for (def, item) in program.items.iter() {
        let Item::Fn(f) = item else { continue };
        let in_entry = entry.is_some_and(|m| program.defs[def].module == DefModule::Source(m));
        if in_entry && !f.exports.is_empty() && !is_entry_adapter(program, def) {
            out.push(HostExports {
                def,
                exports: &f.exports,
                entry: None,
            });
        }
    }
    for e in &program.entry_exports {
        if let Some(Item::Fn(adapter)) = program.item(e.adapter) {
            out.push(HostExports {
                def: e.def,
                exports: &adapter.exports,
                entry: Some(e.span),
            });
        }
    }
    out
}

/// Two exports with the same name for the same target (E0438), reported at the second: at its
/// `@export`, or at the `@entry` that made it.
fn duplicates(program: &Program, entry: Option<ModuleId>, diags: &mut Vec<Diagnostic>) {
    let mut seen: Vec<(&str, &[String])> = Vec::new();
    let mut found = Vec::new();
    for exposed in host_exports(program, entry) {
        for export in exposed.exports {
            let key = (export.target.as_str(), export.args.as_slice());
            if seen.contains(&key) {
                found.push((exposed.entry.unwrap_or(export.span), export));
            } else {
                seen.push(key);
            }
        }
    }
    found.sort_by_key(|(span, _)| (span.module, span.range.start()));
    for (span, export) in found {
        diags.push(Diagnostic {
            span: Some(span),
            kind: DiagnosticKind::DuplicateExport {
                name: SmolStr::new(export.args.last().map_or("", String::as_str)),
                target: export.target.clone(),
            },
        });
    }
}

/// After inference: the parameters and the return value of every `@export` fn are boundary types
/// (E0436, the first parameter of an entry adapter excepted), and its E is empty (E0402). A type
/// error in a fn made by `@entry` becomes one E0440 at the attribute.
pub(crate) fn check(
    program: &Program,
    ty_table: &TyTable,
    sigs: &Signatures,
    diags: &mut Vec<Diagnostic>,
) {
    for (def, item) in program.items.iter() {
        let Item::Fn(f) = item else { continue };
        if f.exports.is_empty() || f.body.is_none() {
            continue;
        }
        let Some(sig) = sigs.get(def) else { continue };
        let Ty::Fn(fn_ty) = &sig.scheme.ty else {
            continue;
        };
        let span = Some(f.exports[0].span);
        let name = program.defs[def].name.clone();
        let skip = usize::from(is_entry_adapter(program, def));
        let show = |ty: &Ty| {
            SmolStr::new(
                ty.display_with(&ty_table.cons, &sig.scheme.params)
                    .to_string(),
            )
        };
        for ty in fn_ty.params.iter().skip(skip) {
            if !is_boundary(program, ty_table, ty) {
                diags.push(Diagnostic {
                    span,
                    kind: DiagnosticKind::NotBoundary {
                        function: name.clone(),
                        ty: show(ty),
                        ret: false,
                    },
                });
            }
        }
        if !is_boundary(program, ty_table, &fn_ty.ret) {
            diags.push(Diagnostic {
                span,
                kind: DiagnosticKind::NotBoundary {
                    function: name.clone(),
                    ty: show(&fn_ty.ret),
                    ret: true,
                },
            });
        }
        export_fails(program, ty_table, sigs, def, span, diags);
    }
    for e in &program.entry_exports {
        export_fails(
            program,
            ty_table,
            sigs,
            e.def,
            Some(body_span(e.span)),
            diags,
        );
        let before = diags.len();
        let body = Some(body_span(e.span));
        diags.retain(|d| d.span != body || d.kind.is_warning());
        if diags.len() != before {
            diags.push(Diagnostic {
                span: Some(e.span),
                kind: DiagnosticKind::EntryMismatch {
                    entry: program.defs[e.entry].name.clone(),
                    adapter: display_name(program, e.adapter, def_module(program, e.entry)),
                },
            });
        }
    }
}

/// E0402 when the E of `def` is not empty. A hidden set variable of a parameter's function type
/// counts (spec 9.4 adds it to the fn's E), so a callback parameter is written with `fails {}`.
fn export_fails(
    program: &Program,
    ty_table: &TyTable,
    sigs: &Signatures,
    def: DefId,
    span: Option<Span>,
    diags: &mut Vec<Diagnostic>,
) {
    let Some(sig) = sigs.get(def) else { return };
    let Ty::Fn(fn_ty) = &sig.scheme.ty else {
        return;
    };
    if fn_ty.fails.elems().is_empty() {
        return;
    }
    diags.push(Diagnostic {
        span,
        kind: DiagnosticKind::ExportFails {
            function: program.defs[def].name.clone(),
            errors: SmolStr::new(
                fn_ty
                    .fails
                    .display_with(&ty_table.cons, &sig.scheme.params)
                    .to_string(),
            ),
        },
    });
}

fn def_module(program: &Program, def: DefId) -> Option<ModuleId> {
    match program.defs[def].module {
        DefModule::Source(m) => Some(m),
        _ => None,
    }
}

/// `[ext.boundary]`: Int, Int64, Float, Bool, String, `()`, an `@external type`, and function
/// types made of these.
fn is_boundary(program: &Program, ty_table: &TyTable, ty: &Ty) -> bool {
    match ty {
        Ty::Prim(
            Prim::Int | Prim::Int64 | Prim::Float | Prim::Bool | Prim::String | Prim::Unit,
        ) => true,
        Ty::Named(con, args) => {
            args.is_empty()
                && matches!(
                    program.item(ty_table.def(*con)),
                    Some(Item::Type(t)) if t.ctor.is_none()
                )
        }
        Ty::Fn(f) => {
            f.params.iter().all(|p| is_boundary(program, ty_table, p))
                && is_boundary(program, ty_table, &f.ret)
        }
        _ => false,
    }
}
