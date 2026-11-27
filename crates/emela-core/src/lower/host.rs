//! The host: external functions (spec 12.1), the bundled files they load, the entry and the
//! functions exported to the host.

use std::path::Path;

use emela_resolve::hir::{DefId, DefKind, DefModule, Item, Span};
use emela_types::Ty;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::ir::{
    Callee, EffectId, Expr, External, ExternalBinding, FnId, HostExport, HostModule, HostRef,
    Module, SetSummary, Type, TypeId,
};
use crate::mono::Monomorphized;

use super::{Exposed, LowerError, Lowerer, Roots};

impl<'a> Lowerer<'a> {
    /// The external function's bindings. A module naming a file becomes the file next to the
    /// declaring source (spec 12.1), or the bundled file for bundled sources.
    pub(super) fn external(&mut self, def: DefId, f: &emela_resolve::hir::FnItem) -> External {
        let module_id = match &self.hir.defs[def].module {
            DefModule::Source(m) => Some(*m),
            _ => None,
        };
        let is_core = module_id.is_some_and(|m| self.modules.is_core(m));
        let dir = module_id
            .and_then(|m| self.paths.get(m))
            .and_then(|p| p.parent());
        let span = self.hir.defs[def].span;
        let mut bindings = Vec::new();
        for e in &f.externals {
            let host = match e.args.as_slice() {
                [name] => HostRef::Global(name.clone()),
                [module, name] => {
                    let module = if !(module.starts_with("./") || module.starts_with("../")) {
                        HostModule::Named(module.clone())
                    } else if is_core {
                        let lib_dir = module_id
                            .map(|m| crate::lib_dir(&self.modules[m].name.to_string()))
                            .unwrap_or("core");
                        HostModule::Bundled(self.bundled_file(lib_dir, module, span))
                    } else if let Some(dir) = dir {
                        HostModule::File(normalize(&dir.join(module)))
                    } else {
                        HostModule::Named(module.clone())
                    };
                    HostRef::Import {
                        module,
                        name: name.clone(),
                    }
                }
                // The parser reported the malformed attribute (E1301).
                _ => continue,
            };
            bindings.push(ExternalBinding {
                target: e.target.to_string(),
                host,
            });
        }
        External { bindings }
    }

    /// Resolves a bundled core file specifier (`./x.mjs`) to its path relative to `lib/`
    /// (`std/x.mjs`). `spec` is relative to the source's directory under `lib/` (`lib_dir`: `std`
    /// or `core`).
    pub(super) fn bundled_file(&mut self, lib_dir: &str, spec: &str, span: Option<Span>) -> String {
        let found = lib_relative(&format!("{lib_dir}/{spec}"))
            .filter(|path| self.bundled_files.iter().any(|(p, _)| p == path));
        match found {
            Some(path) => {
                self.uses_bundled = true;
                path
            }
            None => {
                self.errors.push(LowerError::MissingCoreFile {
                    path: spec.to_owned(),
                    span,
                });
                spec.to_owned()
            }
        }
    }

    /// Reports external functions reachable from `entries` (functions of the monomorphized
    /// module) that lack an `@external` for `target` (spec 12.1), at their HIR definitions.
    pub(super) fn check_target(&mut self, mono: &Monomorphized, entries: &[FnId], target: &str) {
        let missing = unannotated_reachable(&mono.module, entries, target);
        let mut defs: Vec<DefId> = missing
            .into_iter()
            .map(|f| {
                let f = mono.origins[f];
                self.fns
                    .iter()
                    .find(|(_, id)| **id == f)
                    .map(|(&(d, _), _)| d)
                    .expect("IR functions are made from HIR definitions")
            })
            .collect();
        // Copies of one external function for several type arguments are one definition.
        defs.sort();
        defs.dedup();
        // In order of declaration position.
        defs.sort_by_key(|&d| {
            self.hir.defs[d]
                .span
                .map(|s| (s.module.into_raw().into_u32(), s.range.start()))
        });
        for def in defs {
            self.errors.push(LowerError::MissingExternal {
                name: self.hir.defs[def].name.to_string(),
                language: target.to_owned(),
                span: self.hir.defs[def].span,
            });
        }
    }

    /// Whether `def` is the entry module's `main` (not a test function).
    pub(super) fn is_main(&self, def: DefId) -> bool {
        let data = &self.hir.defs[def];
        data.kind == DefKind::Fn
            && self
                .entry
                .is_some_and(|entry| data.module == DefModule::Source(entry))
            && data.name == "main"
            && !self.tests.contains(&def)
    }

    /// When the entry's `main` returns `ExitCode` of `Std.Process` (`[dyn.error]`), the output's
    /// `main` is a wrapper that calls it and passes its value to `exit_with` of `Std.Process`,
    /// which hands the exit code to the host. The `main` of the source keeps its own function,
    /// renamed and no longer exported: the launchers of every target call the `main` they always
    /// call, which returns `()`, and code that calls `main` (`[api.std.process]`: a test) still
    /// gets the value. `main` returns after the releases of its withs have run and the children of
    /// its scopes have ended, so the wrapper sets the exit code after them.
    ///
    /// Declares the wrapper and returns (wrapper, the source's `main`, `exit_with`). The wrapper is
    /// defined by [`Lowerer::define_exit_code_main`] once the source's `main` is lowered.
    pub(super) fn exit_code_main(&mut self) -> Option<(FnId, FnId, FnId)> {
        let hir = self.hir;
        let types = self.types;
        let (&(main_def, _), &main) = self.fns.iter().find(|((def, spec), _)| {
            *spec == 0
                && hir.item(*def).is_some_and(|i| matches!(i, Item::Fn(_)))
                && self.is_main(*def)
        })?;
        let Some(Ty::Fn(sig)) = types.sigs.get(main_def).map(|s| &s.scheme.ty) else {
            return None;
        };
        if !sig.params.is_empty() || !emela_types::is_exit_code(hir, &types.ty_table, &sig.ret) {
            return None;
        }
        let process = hir
            .modules
            .iter()
            .find(|(_, items)| items.name.segments() == ["Std", "Process"])?
            .1;
        let exit_with =
            process.items.iter().copied().find(|&def| {
                hir.defs[def].name == "exit_with" && hir.defs[def].kind == DefKind::Fn
            })?;
        let exit_with = self.fn_id(exit_with, 0);
        // `$` never appears in Emela names, so the new name cannot clash.
        let source = &mut self.module.functions[main];
        source.name = "main$value".to_owned();
        source.exported = false;
        let wrapper = self.module.declare("main", true);
        Some((wrapper, main, exit_with))
    }

    /// `fn main() { exit_with(main$value()) }`, with the E and R of the source's `main`. A generic
    /// `main` (its E is a set type parameter) is called with the type arguments a root gets
    /// (`crate::mono::monomorphize_from`): the empty set for a set, Never for a type.
    pub(super) fn define_exit_code_main(&mut self, (wrapper, main, exit_with): (FnId, FnId, FnId)) {
        let source = &self.module.functions[main];
        let (fails, suspends, set_params, type_params) = (
            source.fails,
            source.suspends,
            source.set_params.clone(),
            source.type_params,
        );
        let takes_ev = self.module.takes_ev(main);
        let empty = self.module.intern(Type::Set(SetSummary::default()));
        let ty_args = (0..type_params)
            .map(|i| {
                if set_params.contains(&i) {
                    empty
                } else {
                    TypeId::NEVER
                }
            })
            .collect();
        let ret = self.module.functions[main].ret;
        let value = self.module.local("code", ret);
        let mut params = Vec::new();
        let mut args = Vec::new();
        if takes_ev {
            let ev = self.ev_local();
            params.push(ev);
            args.push(Expr::Var(ev));
        }
        let body = Expr::Let {
            var: value,
            value: Box::new(Expr::Call {
                callee: Callee::Direct { f: main, ty_args },
                args,
                fails,
                suspends,
            }),
            body: Box::new(Expr::Call {
                callee: Callee::Direct {
                    f: exit_with,
                    ty_args: Vec::new(),
                },
                args: vec![Expr::Var(value)],
                fails: false,
                suspends: false,
            }),
        };
        self.module.define(wrapper, params, TypeId::UNIT, body);
        let f = &mut self.module.functions[wrapper];
        f.fails = fails;
        f.suspends = suspends;
    }

    /// Whether `def` is a root of the build: `main`, a fn exposed to the host, or with
    /// `Roots::Public` a `pub` fn of the entry.
    pub(super) fn is_exported(&self, def: DefId) -> bool {
        self.is_main(def)
            || self.host_exports.iter().any(|e| e.def == def)
            || (self.roots == Roots::Public && self.is_public_root(def))
    }

    /// A root of `Roots::Public`: `main`, or a `pub` fn of the entry that is not a test function
    /// and has no trait-bounded type parameter.
    pub(super) fn is_public_root(&self, def: DefId) -> bool {
        let data = &self.hir.defs[def];
        let bounded = self
            .types
            .sigs
            .get(def)
            .is_some_and(|sig| sig.scheme.params.iter().any(|p| !p.traits.is_empty()));
        self.is_main(def)
            || (data.kind == DefKind::Fn
                && data.is_pub
                && self
                    .entry
                    .is_some_and(|entry| data.module == DefModule::Source(entry))
                && !self.tests.contains(&def)
                && !bounded)
    }

    /// Fills [`Lowerer::host_exports`] with the exports for `language` (every language for
    /// `None`), and reports the ones the build target cannot expose: one that may suspend on the
    /// native target, whose language is `c` (E5202), and on `wasm` any written by a user (`[target.wasm.external]`; only `Std.Http.serve`, the entry
    /// adapter of the bundled `Std`, is exposed by the runtime).
    pub(super) fn collect_host_exports(&mut self, language: Option<&str>) {
        for exposed in emela_types::host_exports(self.hir, self.entry) {
            let entry = self.hir.entry_exports.iter().find(|e| e.def == exposed.def);
            if language == Some("c")
                && let Some(e) = entry.filter(|e| emela_types::is_http_serve(self.hir, e.adapter))
            {
                // `serve` has no `@export(c, ...)`, so nothing would be exposed and the program
                // would build without an entry.
                self.errors
                    .push(LowerError::HttpServiceOnC { span: Some(e.span) });
                continue;
            }
            let exports: Vec<HostExport> = exposed
                .exports
                .iter()
                .filter(|e| language.is_none_or(|l| e.target == l))
                .map(|e| {
                    let (interface, name) = match e.args.as_slice() {
                        [interface, name] => (Some(interface.clone()), name.clone()),
                        [name] => (None, name.clone()),
                        // The parser reported it (E1301).
                        _ => return None,
                    };
                    Some(HostExport {
                        target: e.target.to_string(),
                        interface,
                        name,
                    })
                })
                .collect::<Option<_>>()
                .unwrap_or_default();
            if exports.is_empty() {
                continue;
            }
            let name = self.hir.defs[entry.map_or(exposed.def, |e| e.entry)]
                .name
                .to_string();
            let span = Some(exposed.entry.unwrap_or(exposed.exports[0].span));
            match language {
                Some("c") if self.fn_suspends(exposed.def, 0) => {
                    self.errors
                        .push(LowerError::SuspendingExport { name, span });
                    continue;
                }
                // The runtime exposes the entry adapter of the bundled `Std.Http`.
                Some("wasm")
                    if !entry.is_some_and(|e| emela_types::is_http_serve(self.hir, e.adapter)) =>
                {
                    self.errors.push(LowerError::WasmExport { name, span });
                    continue;
                }
                _ => {}
            }
            self.host_exports.push(Exposed {
                def: exposed.def,
                exports,
            });
        }
    }
}

/// Collapses `.` and `..` in a path textually (symbolic links are not resolved).
pub(super) fn normalize(path: &Path) -> String {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            c => out.push(c),
        }
    }
    out.to_string_lossy().into_owned()
}

/// Turns a specifier relative to `lib/` (`./a/../b.mjs`) into a path from `lib/` with `.` and `..`
/// collapsed (`b.mjs`). `None` if it leaves `lib/`.
pub(super) fn lib_relative(spec: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in spec.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// External functions without an `@external` for `target` that are reachable from `entries`
/// (spec 12.1), in a monomorphized module. A handler operation's implementation (a lambda in
/// `Expr::Handler`) is followed only once the operation is called or used as a value somewhere
/// reachable. Effects with different type arguments are different effect instances after
/// monomorphization, so calling `put` of `Store[String]` does not follow the implementation of
/// `put` of `Store[Int]`.
pub(super) fn unannotated_reachable(m: &Module, entries: &[FnId], target: &str) -> Vec<FnId> {
    #[derive(Default)]
    struct Walk<'m> {
        fns: Vec<FnId>,
        /// (effect instance, operation index) called or used as a value.
        used: FxHashSet<(EffectId, usize)>,
        /// Implementations not followed yet, by (effect instance, operation index).
        pending: FxHashMap<(EffectId, usize), Vec<&'m Expr>>,
        /// Bodies to walk: implementations whose operation became used.
        ready: Vec<&'m Expr>,
    }
    impl<'m> Walk<'m> {
        fn new(entries: &[FnId]) -> Self {
            Walk {
                fns: entries.to_vec(),
                ..Walk::default()
            }
        }

        /// The next function to visit, once every implementation that became used is walked.
        fn next_fn(&mut self) -> Option<FnId> {
            while let Some(body) = self.ready.pop() {
                self.expr(body);
            }
            self.fns.pop()
        }

        fn expr(&mut self, e: &'m Expr) {
            match e {
                Expr::Fn { f, .. }
                | Expr::Call {
                    callee: Callee::Direct { f, .. },
                    ..
                } => self.fns.push(*f),
                Expr::Call {
                    callee: Callee::Op { effect, op, .. },
                    ..
                } => {
                    let key = (*effect, *op);
                    if self.used.insert(key)
                        && let Some(bodies) = self.pending.remove(&key)
                    {
                        self.ready.extend(bodies);
                    }
                }
                Expr::Handler { effect, ops, .. } => {
                    for (i, op) in ops.iter().enumerate() {
                        let key = (*effect, i);
                        if self.used.contains(&key) {
                            self.ready.push(op);
                        } else {
                            self.pending.entry(key).or_default().push(op);
                        }
                    }
                    return;
                }
                _ => {}
            }
            e.for_each_child(&mut |c| self.expr(c));
        }
    }

    let mut seen = vec![false; m.functions.len()];
    let mut walk = Walk::new(entries);
    let mut missing = Vec::new();
    while let Some(f) = walk.next_fn() {
        let i = f.into_raw().into_u32() as usize;
        if std::mem::replace(&mut seen[i], true) {
            continue;
        }
        let func = &m.functions[f];
        if let Some(ext) = &func.external {
            if ext.binding(target).is_none() {
                missing.push(f);
            }
            continue;
        }
        walk.expr(&func.body);
    }
    missing
}
