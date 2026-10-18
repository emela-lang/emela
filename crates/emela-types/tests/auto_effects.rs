//! The effects a target provides to `main` (`check_program_with_auto_main_effects`, spec 16.6):
//! `main`'s R may contain them, and other effects are still reported (E0402).

mod common;

use emela_resolve::hir::DefModule;
use emela_types::{AutoEffect, CheckOptions, DiagnosticKind, check_program_with_auto_main_effects};

use common::resolve_main;

const SRC: &str = "\
effect Led { fn on() -> () }
effect Buzzer { fn beep() -> () }

fn main() use { Led } {
  (use Led).on()
}
";

/// The effects E0402 reports, checking `src` with the effects named `provided` given to `main`.
fn unhandled(src: &str, provided: &[&str]) -> Vec<String> {
    let (program, module, diagnostics) = resolve_main(src);
    assert_eq!(diagnostics, []);
    let def = |name: &str| {
        program
            .defs
            .iter()
            .find(|(_, data)| data.module == DefModule::Source(module) && data.name == name)
            .map(|(def, _)| def)
            .unwrap_or_else(|| panic!("no `{name}`"))
    };
    let options = CheckOptions {
        main: Some(def("main")),
        tests: Vec::new(),
    };
    let auto: Vec<AutoEffect> = provided.iter().map(|n| AutoEffect::new(def(n))).collect();
    let types = check_program_with_auto_main_effects(&program, &options, &auto);
    types
        .diags
        .iter()
        .flat_map(|d| match &d.kind {
            DiagnosticKind::UnhandledEffects { effects, .. } => {
                effects.iter().map(ToString::to_string).collect::<Vec<_>>()
            }
            other => panic!("unexpected diagnostic: {other:?}"),
        })
        .collect()
}

#[test]
fn main_may_use_the_provided_effects() {
    assert_eq!(unhandled(SRC, &["Led"]), Vec::<String>::new());
}

#[test]
fn other_effects_are_still_reported() {
    assert_eq!(unhandled(SRC, &[]), ["Led"]);
    assert_eq!(unhandled(SRC, &["Buzzer"]), ["Led"]);
}
