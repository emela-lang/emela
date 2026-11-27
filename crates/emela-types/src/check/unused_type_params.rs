//! W3002 (`[fn.decl]`, `[trait.decl]`, `[ext.decl]`): a type parameter of a fn that does not
//! appear in the type of the fn. A call determines type parameters only from that type, so such a
//! type parameter is never determined.

use emela_resolve::hir::{DefModule, Item, Program};

use crate::decl::signature::Signatures;
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::ty::{Row, RowElem, Ty};

/// Reports W3002 for top-level fns (including `@external` ones) and for the type parameters that
/// trait functions declare themselves. Called once every body is checked, so a return type, E and
/// R omitted in the declaration are the inferred ones.
///
/// The type parameters written in the declaration are `Bound(0)` to `Bound(n - 1)` of the scheme
/// (after `Self`, which is `Bound(0)` of a trait function), in the order written; the parameters
/// added by generalization and the hidden sets of parameter function types come after them. A
/// bound is not part of the type, so a type parameter that only has a bound is reported. A fn
/// whose type contains a type that could not be read is skipped: the type parameter may be in it.
pub(crate) fn check(program: &Program, sigs: &Signatures, diags: &mut Vec<Diagnostic>) {
    for (def, data) in program.defs.iter() {
        if !matches!(data.module, DefModule::Source(_)) {
            continue;
        }
        let (item, offset) = match program.item(def) {
            // The fns of an impl cannot declare type parameters (`[trait.impl]`).
            Some(Item::Fn(item)) if program.impl_fn(def).is_none() => (item, 0),
            Some(Item::TraitFn(item)) => (item, 1),
            _ => continue,
        };
        if item.sig.type_params.is_empty() {
            continue;
        }
        let Some(sig) = sigs.get(def) else {
            continue;
        };
        let mut used = vec![false; sig.scheme.params.len()];
        let mut broken = false;
        visit(&sig.scheme.ty, &mut used, &mut broken);
        if broken {
            continue;
        }
        for (i, tp) in item.sig.type_params.iter().enumerate() {
            if used.get(i + offset).copied().unwrap_or(true) {
                continue;
            }
            let param = &program.defs[tp.def];
            diags.push(Diagnostic {
                span: param.span,
                kind: DiagnosticKind::UnusedTypeParam {
                    param: param.name.clone(),
                    function: data.name.clone(),
                },
            });
        }
    }
}

/// Marks the type parameters that occur in `ty`, as types (`Ty::Bound`) or as sets
/// (`RowElem::Bound`), and records whether `ty` contains a type that could not be read.
fn visit(ty: &Ty, used: &mut [bool], broken: &mut bool) {
    match ty {
        Ty::Bound(i) => mark(*i, used),
        Ty::Error => *broken = true,
        Ty::Fn(f) => {
            visit_row(&f.fails, used);
            visit_row(&f.uses, used);
        }
        Ty::Set(_, row) => visit_row(row, used),
        _ => {}
    }
    ty.for_each_child(|child| visit(child, used, broken));
}

fn visit_row(row: &Row, used: &mut [bool]) {
    for elem in row.elems() {
        if let RowElem::Bound(i) = elem {
            mark(*i, used);
        }
    }
}

fn mark(i: u32, used: &mut [bool]) {
    if let Some(slot) = used.get_mut(i as usize) {
        *slot = true;
    }
}
