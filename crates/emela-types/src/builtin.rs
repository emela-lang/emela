//! The providers of built-in effects (spec chapter 11).
//!
//! A built-in effect is an `@intrinsic effect` of the bundled sources (`TaskGroup`, `Deferred`).
//! Its provider is the built-in handler (`@intrinsic`) that implements it (`Task.Group`,
//! `Deferred`), and users cannot write another (E4003).
//!
//! Everything here is found from the declarations, not from names, so the checks follow the
//! bundled sources.

use emela_resolve::hir::{DefId, DefModule, Item, Program, Res};
use smol_str::SmolStr;

use crate::lower::is_intrinsic_effect;

/// Facts about the built-in effects of a program.
#[derive(Clone, Debug, Default)]
pub(crate) struct Builtins {
    /// The built-in handlers of built-in effects: (effect, display name).
    pub(crate) handlers: Vec<(DefId, SmolStr)>,
}

impl Builtins {
    pub(crate) fn new(program: &Program) -> Self {
        let effects: Vec<DefId> = program
            .defs
            .iter()
            .map(|(def, _)| def)
            .filter(|&def| is_intrinsic_effect(program, def))
            .collect();
        let mut builtins = Builtins::default();
        if effects.is_empty() {
            return builtins;
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
                builtins.handlers.push((effect, display));
            }
        }
        // A built-in effect opened by a handler member of another (`Permit`, opened by `Permits`
        // of `Semaphore`) is provided by the handler that provides that one.
        for &effect in &effects {
            for &member in program.effect_members(effect) {
                let Some(Item::Member(m)) = program.item(member) else {
                    continue;
                };
                let Some(display) = builtins.provider(effect).cloned() else {
                    continue;
                };
                for r in &m.effects {
                    if let Res::Def(opened) = r.res
                        && effects.contains(&opened)
                        && builtins.provider(opened).is_none()
                    {
                        builtins.handlers.push((opened, display.clone()));
                    }
                }
            }
        }
        builtins
    }

    /// The display name of the built-in handler that provides `effect` (`Task.Group`,
    /// `Deferred`).
    pub(crate) fn provider(&self, effect: DefId) -> Option<&SmolStr> {
        self.handlers
            .iter()
            .find(|(e, _)| *e == effect)
            .map(|(_, d)| d)
    }
}

/// `Task.Group`: the last segment of the module and the definition's name.
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
