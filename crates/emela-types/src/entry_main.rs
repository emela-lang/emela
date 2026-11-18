//! The return type of the entry's `main` (`[dyn.error]`, `[effect.with]`): `()`, or `ExitCode` of
//! `Std.Process` (`[api.std.process]`), whose value chooses the exit code of the program. Any other
//! type, other than Never, is W0304: the value is discarded.

use emela_resolve::hir::{DefModule, Item, Program};
use smol_str::SmolStr;

use crate::check::CheckOptions;
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::lower::TyTable;
use crate::signature::Signatures;
use crate::ty::{Prim, Ty};

/// Whether `ty` is `ExitCode` of the bundled `Std.Process`. `Std` cannot be the root name of a
/// project or a dependency (spec 4.1), so the module name identifies the bundled one.
pub fn is_exit_code(program: &Program, table: &TyTable, ty: &Ty) -> bool {
    let Ty::Named(con, args) = ty else {
        return false;
    };
    let data = &program.defs[table.def(*con)];
    args.is_empty()
        && data.name == "ExitCode"
        && matches!(data.module, DefModule::Source(m)
            if program.modules.get(m).is_some_and(|items| items.name.segments() == ["Std", "Process"]))
}

/// Reports W0304 for the entry's `main` ([`CheckOptions::main`]) when it returns a type other than
/// `()`, `ExitCode` and Never. Decided from the return type fixed by inference, whether written or
/// inferred.
pub(crate) fn check(
    program: &Program,
    table: &TyTable,
    sigs: &Signatures,
    options: &CheckOptions,
    diags: &mut Vec<Diagnostic>,
) {
    let Some(main) = options.main else {
        return;
    };
    let Some(Ty::Fn(f)) = sigs.get(main).map(|sig| &sig.scheme.ty) else {
        return;
    };
    match &f.ret {
        // A broken type is reported where it was made.
        Ty::Prim(Prim::Unit) | Ty::Never | Ty::Error => return,
        // A type parameter of main (the result of a body that only panics) has no value: the entry
        // is instantiated with Never for it (`emela_core::monomorphize_from`).
        Ty::Bound(_) | Ty::Var(_) => return,
        ty if is_exit_code(program, table, ty) => return,
        _ => {}
    }
    // The annotation of the return type, or the name of main when the type is inferred.
    let annotation = match program.item(main) {
        Some(Item::Fn(item)) => item.sig.ret.and_then(|ty| program.types[ty].span),
        _ => None,
    };
    diags.push(Diagnostic {
        span: annotation.or(program.defs[main].span),
        kind: DiagnosticKind::MainValueDiscarded {
            ty: SmolStr::new(f.ret.display(&table.cons).to_string()),
        },
    });
}
