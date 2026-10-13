//! Built-in effects and the scope function of fibers (spec chapter 11).
//!
//! A built-in effect is an `@intrinsic effect` of the bundled sources (`Scope`). Its provider is
//! the `@intrinsic` fn of the same module whose parameter function type names the effect in its
//! `use` clause (`Fiber.scope`). The types the effect's operations return (`Fiber`, `Deferred`)
//! are the handles that cannot leave the provider (E0425, spec 11.8).
//!
//! Everything here is found from the declarations, not from names, so the checks follow the
//! bundled sources.

use emela_resolve::hir::{DefId, DefKind, DefModule, Item, Program, Res, TypeId, TypeKind};
use smol_str::SmolStr;

use crate::lower::is_intrinsic_effect;

/// The provider of a built-in effect: a scope function like `Fiber.scope`.
#[derive(Clone, Debug)]
pub(crate) struct ScopeFn {
    pub(crate) def: DefId,
    /// The effect it provides.
    pub(crate) effect: DefId,
    /// Shown in diagnostics: the last segment of the module and the fn's name (`Fiber.scope`).
    pub(crate) display: SmolStr,
}

/// Facts about the built-in effects of a program.
#[derive(Clone, Debug, Default)]
pub(crate) struct Fibers {
    pub(crate) scope_fns: Vec<ScopeFn>,
    /// Named types the operations of built-in effects return (`Fiber`, `Deferred`).
    pub(crate) handles: Vec<DefId>,
}

impl Fibers {
    pub(crate) fn new(program: &Program) -> Self {
        let effects: Vec<DefId> = program
            .defs
            .iter()
            .map(|(def, _)| def)
            .filter(|&def| is_intrinsic_effect(program, def))
            .collect();
        let mut fibers = Fibers::default();
        if effects.is_empty() {
            return fibers;
        }
        for &effect in &effects {
            let Some(Item::Effect(e)) = program.item(effect) else {
                continue;
            };
            for &op in &e.ops {
                let Some(Item::Op(o)) = program.item(op) else {
                    continue;
                };
                if let Some(ret) = o.sig.ret
                    && let TypeKind::Path {
                        res: Res::Def(ty), ..
                    } = program.types[ret].kind
                    && program.defs[ty].kind == DefKind::Type
                    && !fibers.handles.contains(&ty)
                {
                    fibers.handles.push(ty);
                }
            }
        }
        for (def, data) in program.defs.iter() {
            let Some(Item::Fn(f)) = program.item(def) else {
                continue;
            };
            if !f.is_intrinsic
                || !effects
                    .iter()
                    .any(|&e| program.defs[e].module == data.module)
            {
                continue;
            }
            let provided = f
                .sig
                .params
                .iter()
                .filter_map(|p| p.ty)
                .find_map(|ty| names_effect(program, ty, &effects));
            if let Some(effect) = provided {
                fibers.scope_fns.push(ScopeFn {
                    def,
                    effect,
                    display: qualified_name(program, def),
                });
            }
        }
        fibers
    }

    /// The display name of the function that provides `effect` (`Fiber.scope`).
    pub(crate) fn provider(&self, effect: DefId) -> Option<&SmolStr> {
        self.scope_fns
            .iter()
            .find(|s| s.effect == effect)
            .map(|s| &s.display)
    }
}

/// The built-in effect named in the `use` clause of the function type `ty`, if any.
fn names_effect(program: &Program, ty: TypeId, effects: &[DefId]) -> Option<DefId> {
    let TypeKind::Fn { uses, .. } = &program.types[ty].kind else {
        return None;
    };
    uses.iter()
        .flat_map(|u| &u.items)
        .find_map(|item| match item.res {
            Res::Def(d) if effects.contains(&d) => Some(d),
            _ => None,
        })
}

/// `Fiber.scope`: the last segment of the module and the definition's name.
fn qualified_name(program: &Program, def: DefId) -> SmolStr {
    let data = &program.defs[def];
    match data.module {
        DefModule::Source(m) => match program.modules.get(m) {
            Some(items) if !items.name.segments().is_empty() => {
                SmolStr::new(format!("{}.{}", items.name.last(), data.name))
            }
            _ => data.name.clone(),
        },
        _ => data.name.clone(),
    }
}
