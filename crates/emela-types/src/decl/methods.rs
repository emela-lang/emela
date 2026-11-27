//! The functions a method call `x.f(...)` can call (`[type.rules]`): the fns of the module that
//! defines the type of x whose first parameter has that type at its head, and the functions that
//! take `self` of the traits that the type implements, among those visible by name. Shared by type
//! inference (`body/methods.rs`) and completion after `.` (`[lsp.completion]`).

use emela_resolve::hir::{DefId, DefKind, DefModule, Item, Program};
use emela_resolve::{ModuleId, PRELUDE};

use crate::decl::signature::Signatures;
use crate::decl::traits::Traits;
use crate::ty::{Ty, TyParam};

/// The module that defines the type whose head is `head`. The built-in types and the types of
/// the Prelude have their functions in the core module of the same name (`List`, `String`,
/// `Option`).
pub(crate) fn defining_module(program: &Program, head: DefId) -> Option<ModuleId> {
    let data = &program.defs[head];
    if let DefModule::Source(m) = data.module {
        let items = &program.modules[m];
        if !(items.is_core && items.name.segments() == [PRELUDE]) {
            return Some(m);
        }
    }
    program
        .modules
        .iter()
        .find(|(_, m)| m.is_core && m.name.segments() == [data.name.clone()])
        .map(|(id, _)| id)
}

/// The traits looked up for a value of type `ty` in `module`: those visible by name there and,
/// for a type parameter, the traits of its bounds and their supertraits (a bound includes its
/// supertraits, `[trait.call]`). Whether `ty` implements them is not checked.
pub(crate) fn candidate_traits(
    program: &Program,
    traits: &Traits,
    module: ModuleId,
    ty: &Ty,
    env: &[TyParam],
) -> Vec<DefId> {
    let mut found = program.modules[module].traits.clone();
    if let Ty::Bound(i) = ty {
        let mut stack = env
            .get(*i as usize)
            .map_or_else(Vec::new, |p| p.traits.clone());
        while let Some(t) = stack.pop() {
            if !found.contains(&t) {
                found.push(t);
                stack.extend(traits.supertraits(t).iter().copied());
            }
        }
    }
    found
}

/// The functions of a trait, in declaration order.
pub(crate) fn trait_fns(program: &Program, trait_def: DefId) -> Vec<DefId> {
    match program.item(trait_def) {
        Some(Item::Trait(item)) => item.fns.clone(),
        // The traits of the Prelude have no source.
        _ => program
            .defs
            .iter()
            .filter(|(_, d)| d.kind == DefKind::TraitFn && d.parent == Some(trait_def))
            .map(|(f, _)| f)
            .collect(),
    }
}

/// Whether the trait function `f` takes `self` as its first parameter.
pub(crate) fn takes_self(sigs: &Signatures, f: DefId) -> bool {
    sigs.get(f)
        .and_then(|sig| sig.labels.first())
        .is_some_and(|label| label.as_deref() == Some("self"))
}

/// The type with its type arguments replaced by `Ty::Error`, which satisfies every trait.
/// Whether a type implements a trait for a method call does not depend on its type arguments: a
/// `List[fn() -> Int]` finds `Show.show`, and the call is E3302.
pub(crate) fn erase_args(ty: &Ty) -> Ty {
    match ty {
        Ty::Named(id, args) => Ty::Named(*id, vec![Ty::Error; args.len()]),
        Ty::List(_) => Ty::list(Ty::Error),
        Ty::Tuple(elems) => Ty::Tuple(vec![Ty::Error; elems.len()]),
        _ => ty.clone(),
    }
}
