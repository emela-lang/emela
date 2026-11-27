//! Snapshot tests for effect set inference (spec chapter 8, 9.2 to 9.5), handler checks, and the
//! restrictions on `@external` and capability values.
//!
//! `render_effects` prints, besides the `render_bodies` output (declared types and diagnostics),
//! the expressions of `main.emel` with a non-empty R, in source order, as `expr: R`. A fn named
//! `main` is checked as `main` (its R must be empty, spec 8.3).

use crate::common;

use std::fmt::Write;

use emela_resolve::hir::{DefKind, DefModule};
use emela_types::{CheckOptions, Supplier, check_program_with};

use common::{describe, resolve_main};

mod declaration_sets;
mod external;
mod handlers;
mod init_arguments;
mod layers;
mod sets;
mod suspend;
mod type_params;

pub(super) fn render_effects(src: &str) -> String {
    render(src, false)
}

/// `render_effects` plus handler init E, `with` initialization orders, and layer expansions.
pub(super) fn render_handlers(src: &str) -> String {
    render(src, true)
}

fn render(src: &str, handlers: bool) -> String {
    let (program, module, resolve_diagnostics) = resolve_main(src);
    let in_main =
        |def: emela_resolve::hir::DefId| program.defs[def].module == DefModule::Source(module);
    let main = program
        .defs
        .iter()
        .find(|(def, data)| in_main(*def) && data.kind == DefKind::Fn && data.name == "main")
        .map(|(def, _)| def);
    let tests = program
        .defs
        .iter()
        .filter(|(def, data)| in_main(*def) && data.name.starts_with("test_"))
        .map(|(def, _)| def)
        .collect();
    let types = check_program_with(
        &program,
        &CheckOptions {
            entry: None,
            main,
            tests,
        },
    );
    let cons = &types.ty_table.cons;

    let mut out = String::new();
    for (def, data) in program.defs.iter() {
        if !in_main(def) || data.kind != DefKind::Fn {
            continue;
        }
        match types.sigs.get(def) {
            Some(sig) => writeln!(out, "{}: {}", data.name, sig.scheme.display(cons)).unwrap(),
            None => writeln!(out, "{}: <no type>", data.name).unwrap(),
        }
    }
    for (def, row) in types.handler_uses.iter() {
        if in_main(def) {
            writeln!(
                out,
                "handler {}: use {}",
                program.defs[def].name,
                row.display_effects(cons, &[])
            )
            .unwrap();
        }
    }
    if handlers {
        for (def, row) in types.handler_fails.iter() {
            if in_main(def) && !row.is_empty() {
                writeln!(
                    out,
                    "handler {}: init fails {}",
                    program.defs[def].name,
                    row.display(cons)
                )
                .unwrap();
            }
        }
        for (def, layer) in types.layers.iter() {
            if in_main(def) {
                let names: Vec<&str> = layer
                    .handlers
                    .iter()
                    .map(|&h| program.defs[h].name.as_str())
                    .collect();
                writeln!(
                    out,
                    "layer {}: [{}] use {}",
                    program.defs[def].name,
                    names.join(", "),
                    layer.uses.display_effects(cons, &[])
                )
                .unwrap();
            }
        }
        // The bundled std has withs of its own (`Std.Http.serve`); only main's are shown.
        let mut withs: Vec<_> = types
            .with_handlers
            .iter()
            .filter(|(expr, _)| program.exprs[*expr].span.module == module)
            .map(|(expr, hs)| (program.exprs[expr].span.range, hs))
            .collect();
        withs.sort_by_key(|(range, _)| range.start());
        for (range, hs) in withs {
            let text = &src[usize::from(range.start())..usize::from(range.end())];
            let head = text.split('{').next().unwrap_or(text).trim();
            let names: Vec<String> = hs
                .binds
                .iter()
                .map(|b| {
                    let name = |def: emela_resolve::hir::DefId| program.defs[def].name.to_string();
                    match &b.supplier {
                        Supplier::Handler(h) if program.label_base(b.effect.def).is_some() => {
                            format!("{} by {}", name(b.effect.def), name(*h))
                        }
                        Supplier::Handler(h) => name(*h),
                        Supplier::Effect(t) => format!("{} <- {}", name(b.effect.def), name(t.def)),
                        Supplier::Member { member, .. } => format!("open {}", name(*member)),
                    }
                })
                .collect();
            let defaults: Vec<String> = hs
                .defaults
                .iter()
                .map(|(t, i)| format!(" {} from {i}", program.defs[t.def].name))
                .collect();
            writeln!(
                out,
                "{head}: init [{}]{}",
                names.join(", "),
                defaults.concat()
            )
            .unwrap();
        }
    }
    for d in &resolve_diagnostics {
        writeln!(out, "resolve: {d}").unwrap();
    }
    let mut diags: Vec<_> = types
        .diags
        .iter()
        .filter(|d| d.span.is_some_and(|s| s.module == module))
        .collect();
    diags.sort_by_key(|d| d.span.map(|s| s.range.start()));
    for d in diags {
        writeln!(out, "{}", describe(src, cons, d)).unwrap();
    }

    let mut rows: Vec<_> = types
        .effects
        .iter()
        .filter(|(_, row)| !row.is_empty())
        .filter_map(|(expr, row)| {
            let span = program.exprs[expr].span;
            (span.module == module).then_some((span.range, row))
        })
        .collect();
    rows.sort_by_key(|(range, _)| (range.start(), std::cmp::Reverse(range.end())));
    out.push_str("--- R\n");
    for (range, row) in rows {
        let text = &src[usize::from(range.start())..usize::from(range.end())];
        let text = text.replace('\n', " ");
        writeln!(out, "{text}: {}", row.display_effects(cons, &[])).unwrap();
    }
    out
}
