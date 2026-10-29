//! The functions a method call `x.f(...)` can call (`[type.rules]`): the fns of the module that
//! defines the type of x whose first parameter has that type at its head, and the functions that
//! take `self` of the traits that the type implements, among those visible by name. Shared by type
//! inference (`body/methods.rs`) and completion after `.` (`[lsp.completion]`).

use emela_resolve::hir::{DefId, DefKind, DefModule, Item, Program};
use emela_resolve::{ModuleId, PRELUDE};

use crate::check::ProgramTypes;
use crate::signature::Signatures;
use crate::traits::{Traits, head_def};
use crate::ty::{Ty, TyParam};

/// The functions that can be called as methods on a value of type `ty` from `module`, with any
/// name, once the program is checked: the fns of the module of its head first, in declaration
/// order, then the functions of the traits. Empty unless the head of `ty` is a named type
/// (`[lsp.completion]`). `env` are the type parameters `Ty::Bound(i)` stands for.
pub fn methods_of(
    program: &Program,
    types: &ProgramTypes,
    module: ModuleId,
    ty: &Ty,
    env: &[TyParam],
) -> Vec<DefId> {
    let Some(head) = head_def(program, &types.ty_table, ty) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    if let Some(owner) = defining_module(program, head) {
        for &def in &program.modules[owner].items {
            let data = &program.defs[def];
            let first = match types.sigs.get(def).map(|sig| &sig.scheme.ty) {
                Some(Ty::Fn(f)) => f.params.first(),
                _ => None,
            };
            if data.kind == DefKind::Fn
                && (data.is_pub || owner == module)
                && first.and_then(|t| head_def(program, &types.ty_table, t)) == Some(head)
            {
                found.push(def);
            }
        }
    }
    for t in candidate_traits(program, &types.traits, module, ty, env) {
        if types
            .traits
            .satisfies(program, &types.ty_table, &erase_args(ty), t, env)
            .is_ok()
        {
            found.extend(
                trait_fns(program, t)
                    .into_iter()
                    .filter(|&f| takes_self(&types.sigs, f)),
            );
        }
    }
    found
}

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
/// `List[fn() -> Int]` finds `Show.show`, and the call is E0326.
pub(crate) fn erase_args(ty: &Ty) -> Ty {
    match ty {
        Ty::Named(id, args) => Ty::Named(*id, vec![Ty::Error; args.len()]),
        Ty::List(_) => Ty::list(Ty::Error),
        Ty::Tuple(elems) => Ty::Tuple(vec![Ty::Error; elems.len()]),
        _ => ty.clone(),
    }
}
