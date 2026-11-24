//! Built-in effects and the scope function of fibers (spec chapter 11).
//!
//! A built-in effect is an `@intrinsic effect` of the bundled sources (`Scope`). Its provider is
//! the `@intrinsic` fn of the same module whose parameter function type names the effect in its
//! `uses` clause (`Fiber.scope`).
//!
//! Everything here is found from the declarations, not from names, so the checks follow the
//! bundled sources.

use emela_resolve::hir::{DefId, DefModule, Item, Program, Res, TypeId, TypeKind};
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
    /// The built-in handlers of built-in effects: (effect, display name).
    pub(crate) handlers: Vec<(DefId, SmolStr)>,
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
        for (def, data) in program.defs.iter() {
            // A built-in handler provides the one built-in effect it implements (`Deferred`,
            // `[task.deferred]`). One with the name of its effect is shown by that name alone.
            if let Some(Item::Handler(h)) = program.item(def)
                && h.is_intrinsic
                && let [effect] = h.effects.as_slice()
                && let Res::Def(effect) = effect.res
                && effects.contains(&effect)
            {
                let display = if program.twin_handler(effect) == Some(def) {
                    data.name.clone()
                } else {
                    qualified_name(program, def)
                };
                fibers.handlers.push((effect, display));
                continue;
            }
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
        // A built-in effect opened by a handler member of another (`Permit`, opened by `Permits`
        // of `Semaphore`) is provided by the handler that provides that one.
        for &effect in &effects {
            for &member in program.effect_members(effect) {
                let Some(Item::Member(m)) = program.item(member) else {
                    continue;
                };
                let Some(display) = fibers.provider(effect).cloned() else {
                    continue;
                };
                for r in &m.effects {
                    if let Res::Def(opened) = r.res
                        && effects.contains(&opened)
                        && fibers.provider(opened).is_none()
                    {
                        fibers.handlers.push((opened, display.clone()));
                    }
                }
            }
        }
        fibers
    }

    /// The display name of the function or the built-in handler that provides `effect`
    /// (`Fiber.scope`, `Deferred`).
    pub(crate) fn provider(&self, effect: DefId) -> Option<&SmolStr> {
        self.scope_fns
            .iter()
            .find(|s| s.effect == effect)
            .map(|s| &s.display)
            .or_else(|| {
                self.handlers
                    .iter()
                    .find(|(e, _)| *e == effect)
                    .map(|(_, d)| d)
            })
    }
}

/// The built-in effect named in the `uses` clause of the function type `ty`, if any.
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
pub(crate) fn qualified_name(program: &Program, def: DefId) -> SmolStr {
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
