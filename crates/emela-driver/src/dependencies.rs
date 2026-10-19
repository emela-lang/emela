//! Dependencies (spec 4.7): adds the modules of the Pomes the project depends on to the module
//! map, gives each depending side's root names, and checks those names and `[pome].emela`.

use std::collections::BTreeMap;
use std::path::Path;

use emela_resolve::{ModuleMap, ModuleName, PRELUDE, STD, SourceFs, is_type_name, to_type_name};
use smol_str::SmolStr;

use crate::code;
use crate::diagnostic::{Diagnostic, HELP_PREFIX, Location};
use crate::pipeline::{Dependencies, DependencyPome, DependencyRef, PROJECT_FILE, SOURCE_DIR};
use crate::target::Target;

/// The compiler version `[pome].emela` is compared with.
const COMPILER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) fn add(
    fs: &dyn SourceFs,
    modules: &mut ModuleMap,
    dependencies: &Dependencies,
) -> (Vec<Diagnostic>, Vec<emela_resolve::Diagnostic>) {
    let mut diagnostics = Vec::new();
    let mut resolve_diagnostics = Vec::new();
    let mut ids = Vec::new();
    for pome in &dependencies.pomes {
        let manifest = pome.dir.join(PROJECT_FILE);
        let preferred = default_root_name(pome).unwrap_or_else(|| SmolStr::new("Pome"));
        let (id, found) = modules.add_package(
            fs,
            pome.name.as_str().into(),
            preferred,
            &pome.dir.join(SOURCE_DIR),
        );
        resolve_diagnostics.extend(found);
        ids.push(id);
        if dependencies.allow_external.contains(&pome.name) {
            modules.allow_external(id);
        }
        if let Some(emela) = &pome.emela {
            diagnostics.extend(check_compiler_version(pome, emela, &manifest));
        }
    }

    for allowed in &dependencies.allow_external {
        if !dependencies.pomes.iter().any(|p| &p.name == allowed) {
            diagnostics.push(
                Diagnostic::warning(format!(
                    "`{allowed}` is allowed in [external] but not depended on"
                ))
                .with_code(code::UNUSED_EXTERNAL_ALLOW)
                .at(Location::Path(dependencies.manifest.clone())),
            );
        }
    }

    // The project's own top-level names: its modules' first segments.
    let own: Vec<SmolStr> = modules
        .iter()
        .filter(|&(id, _)| !modules.is_core(id) && !modules.is_dependency(id))
        .map(|(_, m)| m.name.segments()[0].clone())
        .collect();
    // `[dependencies]` and `[dev-dependencies]` share the root names of the project.
    let refs: Vec<DependencyRef> = dependencies
        .project
        .iter()
        .chain(&dependencies.dev)
        .cloned()
        .collect();
    let names = root_names(
        &refs,
        &dependencies.pomes,
        &own,
        modules,
        &dependencies.manifest,
        &mut diagnostics,
    );
    let (dev, names): (Vec<_>, Vec<_>) = names
        .into_iter()
        .partition(|(_, p)| dependencies.dev.iter().any(|d| d.pome == *p));
    modules.set_dependencies(None, names.into_iter().map(|(n, p)| (n, ids[p])));
    modules.set_dev_dependencies(dev.into_iter().map(|(n, p)| (n, ids[p])));

    for (i, pome) in dependencies.pomes.iter().enumerate() {
        let own: Vec<SmolStr> = modules
            .package(ids[i])
            .local_names()
            .map(|n| n.segments()[0].clone())
            .collect();
        let manifest = pome.dir.join(PROJECT_FILE);
        let names = root_names(
            &pome.dependencies,
            &dependencies.pomes,
            &own,
            modules,
            &manifest,
            &mut diagnostics,
        );
        modules.set_dependencies(Some(ids[i]), names.into_iter().map(|(n, p)| (n, ids[p])));
    }
    (diagnostics, resolve_diagnostics)
}

/// The root name the author gives a Pome: `[pome].module`, else its last element in PascalCase
/// (`json-codec` → `JsonCodec`). `None` if the last element cannot be converted.
fn default_root_name(pome: &DependencyPome) -> Option<SmolStr> {
    match &pome.module {
        Some(m) => Some(SmolStr::new(m)),
        None => to_type_name(&pome.last_element.replace('-', "_")).ok(),
    }
}

// The root names one Pome (or the project) gives its dependencies, checked: a type name, not
// `Std`, `Prelude` or a core module, and not the same as its own modules or another dependency.
fn root_names(
    refs: &[DependencyRef],
    pomes: &[DependencyPome],
    own: &[SmolStr],
    modules: &ModuleMap,
    manifest: &Path,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<(SmolStr, usize)> {
    let at = || Location::Path(manifest.to_path_buf());
    let mut names: BTreeMap<SmolStr, usize> = BTreeMap::new();
    for r in refs {
        let pome = &pomes[r.pome];
        let name = match (&r.module, default_root_name(pome)) {
            (Some(m), _) => SmolStr::new(m),
            (None, Some(n)) => n,
            (None, None) => {
                diagnostics.push(
                    Diagnostic::error(format!(
                        "cannot make a root name from `{}` of `{}`",
                        pome.last_element, pome.name
                    ))
                    .with_code(code::INVALID_ROOT_NAME)
                    .with_note(format!(
                        "give it one with `module` in the entry of `{}`",
                        pome.name
                    ))
                    .at(at()),
                );
                continue;
            }
        };
        let error = |message: String, code: &'static str| {
            Diagnostic::error(message)
                .with_code(code)
                .with_note(format!(
                    "give `{}` another root name with `module` in its entry",
                    pome.name
                ))
                .at(at())
        };
        if !is_type_name(&name) {
            diagnostics.push(error(
                format!(
                    "`{name}` (the root name of `{}`) is not a module name",
                    pome.name
                ),
                code::INVALID_ROOT_NAME,
            ));
        } else if name == STD || name == PRELUDE || is_core_module(modules, &name) {
            diagnostics.push(error(
                format!(
                    "`{name}` is reserved and cannot be the root name of `{}`",
                    pome.name
                ),
                code::RESERVED_MODULE_NAME,
            ));
        } else if own.contains(&name) {
            diagnostics.push(error(
                format!(
                    "the root name `{name}` of `{}` is also a module of this Pome",
                    pome.name
                ),
                code::DUPLICATE_MODULE,
            ));
        } else if let Some(&other) = names.get(&name) {
            diagnostics.push(error(
                format!(
                    "`{}` and `{}` have the same root name `{name}`",
                    pomes[other].name, pome.name
                ),
                code::DUPLICATE_MODULE,
            ));
        } else {
            names.insert(name, r.pome);
        }
    }
    names.into_iter().collect()
}

fn is_core_module(modules: &ModuleMap, name: &str) -> bool {
    modules
        .lookup(&ModuleName::new([name]))
        .is_some_and(|id| modules.is_core(id))
}

// `[pome].emela` newer than the compiler is an error; an older minor is a warning (the Pome is
// still built). Versions are compared as major.minor.patch, missing parts as 0.
fn check_compiler_version(
    pome: &DependencyPome,
    emela: &str,
    manifest: &Path,
) -> Option<Diagnostic> {
    let parse = |v: &str| -> Option<(u64, u64, u64)> {
        let mut parts = v.split('.').map(|p| p.parse::<u64>());
        let major = parts.next()?.ok()?;
        let minor = parts.next().transpose().ok()?.unwrap_or(0);
        let patch = parts.next().transpose().ok()?.unwrap_or(0);
        Some((major, minor, patch))
    };
    let compiler = parse(COMPILER_VERSION.split('-').next().unwrap_or_default())?;
    let Some(required) = parse(emela) else {
        return Some(
            Diagnostic::error(format!(
                "`[pome].emela` of `{}` is not a version: `{emela}`",
                pome.name
            ))
            .with_code(code::NEWER_COMPILER_NEEDED)
            .at(Location::Path(manifest.to_path_buf())),
        );
    };
    if required > compiler {
        return Some(
            Diagnostic::error(format!(
                "`{}` needs emela {emela} or newer; this is {COMPILER_VERSION}",
                pome.name
            ))
            .with_code(code::NEWER_COMPILER_NEEDED)
            .at(Location::Path(manifest.to_path_buf())),
        );
    }
    ((required.0, required.1) < (compiler.0, compiler.1)).then(|| {
        Diagnostic::warning(format!(
            "`{}` is written for emela {emela}; this is {COMPILER_VERSION}",
            pome.name
        ))
        .with_code(code::OLDER_COMPILER_DEPENDENCY)
        .with_note(
            "it is built anyway; errors inside it may come from changes between the versions",
        )
        .at(Location::Path(manifest.to_path_buf()))
    })
}

/// Dependencies whose `[pome].targets` does not include `target` (E0238, spec 4.7). Only build,
/// run and test have a target; check and the language server do not call this.
pub fn check_dependency_targets(dependencies: &Dependencies, target: Target) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (i, pome) in dependencies.pomes.iter().enumerate() {
        let Some(targets) = &pome.targets else {
            continue;
        };
        if targets.iter().any(|t| t == target.name()) {
            continue;
        }
        let required_by = if dependencies.project.iter().any(|r| r.pome == i) {
            "this Pome".to_string()
        } else {
            dependencies
                .pomes
                .iter()
                .find(|p| p.dependencies.iter().any(|r| r.pome == i))
                .map_or_else(|| "this Pome".to_string(), |p| format!("`{}`", p.name))
        };
        diagnostics.push(
            Diagnostic::error(format!(
                "`{}` supports only {}, not {}",
                pome.name,
                targets.join(", "),
                target.name()
            ))
            .with_code(code::UNSUPPORTED_TARGET)
            .with_note(format!("required by {required_by}"))
            .with_note(format!(
                "{HELP_PREFIX}build for {}, or move the {} entry to a Pome that does not depend on it",
                targets.join(" or "),
                target.name()
            ))
            .at(Location::Path(pome.dir.join(PROJECT_FILE))),
        );
    }
    diagnostics
}
