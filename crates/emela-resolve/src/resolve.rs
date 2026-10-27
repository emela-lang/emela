//! The entry point of name resolution: collects declarations, resolves imports, and lowers
//! declaration contents to HIR.
//!
//! Three phases:
//!
//! 1. Collect declarations: per module, register definitions in separate namespaces (types,
//!    constructors, values)
//! 2. Imports: once every module's declarations are in, bring the listed names and modules into
//!    scope
//! 3. Lower: turn declaration contents (types, expressions, patterns) into HIR ([`crate::lower`])
//!
//! Namespaces are chosen by position: left of `.` only modules and traits are looked up, in type
//! positions only types, and for type names in expressions and patterns only constructors. So a
//! module `Email` and a type `Email` inside it (spec 4.4) do not collide.

use emela_syntax::ast::{self, AstNode, HasAttrs};
use emela_syntax::{SyntaxNode, SyntaxToken};
use la_arena::ArenaMap;
use rowan::TextRange;
use rustc_hash::{FxHashMap, FxHashSet};
use smol_str::SmolStr;

use crate::builtins::{BuiltinKind, BuiltinModules, build_prelude};
use crate::diagnostic::{Diagnostic, DiagnosticKind, Expected};
use crate::hir::{
    DefData, DefId, DefKind, DefModule, ModuleItems, NameRef, Program, RefTarget, Span,
};
use crate::source::{ModuleId, ModuleMap};
use crate::suggest::best_match;
use crate::{Import, ModuleName};

/// Module names live separately, in [`ModuleScope::modules`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Ns {
    /// Type names: type, enum, error, effect, handler, layer, trait.
    Type = 0,
    /// Type names in expressions and patterns: constructors, variants, errors, handlers with
    /// fields.
    Ctor = 1,
    /// Lowercase and uppercase names: fn and const.
    Value = 2,
}

const NAMESPACES: [Ns; 3] = [Ns::Type, Ns::Ctor, Ns::Value];

/// A module that can be written left of `.`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ModRef {
    Source(ModuleId),
    Builtin(SmolStr),
}

#[derive(Debug, Clone)]
pub(crate) struct Entry<T> {
    pub(crate) target: T,
    /// The range of the definition's name or of the import's name.
    pub(crate) range: Option<TextRange>,
    /// Brought in by an import; not visible to other modules.
    pub(crate) imported: bool,
}

#[derive(Debug, Default)]
pub(crate) struct ModuleScope {
    names: [FxHashMap<SmolStr, Entry<DefId>>; 3],
    pub(crate) modules: FxHashMap<SmolStr, Entry<ModRef>>,
}

impl ModuleScope {
    pub(crate) fn get(&self, ns: Ns, name: &str) -> Option<&Entry<DefId>> {
        self.names[ns as usize].get(name)
    }

    pub(crate) fn names(&self, ns: Ns) -> impl Iterator<Item = &SmolStr> {
        self.names[ns as usize].keys()
    }
}

/// A declaration handed to the lowering phase.
pub(crate) struct Pending {
    pub(crate) module: ModuleId,
    pub(crate) def: DefId,
    pub(crate) item: ast::Item,
    /// Variants, effect operations and trait functions, in declaration order.
    pub(crate) children: Vec<DefId>,
    /// The constructor of a type or handler.
    pub(crate) ctor: Option<DefId>,
}

/// What the name left of `.` resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DotLeft {
    Module(ModRef),
    Trait(DefId),
}

pub(crate) struct Resolver<'a> {
    pub(crate) modules: &'a ModuleMap,
    builtins: &'a dyn BuiltinModules,
    builtin_modules: Vec<SmolStr>,
    builtin_defs: FxHashMap<(SmolStr, SmolStr), DefId>,
    pub(crate) program: Program,
    pub(crate) scopes: ArenaMap<ModuleId, ModuleScope>,
    prelude: [FxHashMap<SmolStr, DefId>; 3],
    /// Trait functions and effect operations, looked up by name.
    pub(crate) members: FxHashMap<DefId, Vec<DefId>>,
    /// Effects declared as labels (`effect AuditLog of Console`, issue #255).
    pub(crate) labels: FxHashSet<DefId>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

/// Modules missing from `files` (files that could not be read) are treated as empty.
///
/// Diagnostics are ordered by module, and by position within a module.
pub fn resolve(
    modules: &ModuleMap,
    files: &ArenaMap<ModuleId, ast::SourceFile>,
    builtins: &dyn BuiltinModules,
) -> (Program, Vec<Diagnostic>) {
    let mut resolver = Resolver::new(modules, builtins);
    let mut pending = Vec::new();
    for (module, data) in modules.iter() {
        resolver.scopes.insert(module, ModuleScope::default());
        resolver.program.modules.insert(
            module,
            ModuleItems {
                name: data.name.clone(),
                items: Vec::new(),
                is_core: modules.is_core(module),
            },
        );
        if let Some(file) = files.get(module) {
            pending.extend(resolver.collect(module, file));
        }
    }
    resolver.use_core_prelude();
    for (module, _) in modules.iter() {
        if let Some(file) = files.get(module) {
            resolver.imports(module, file);
        }
        if !modules.is_core(module) {
            resolver.warn_prelude_shadowing(module);
        }
    }
    for pending in pending {
        crate::lower::lower_item(&mut resolver, pending);
    }
    let Resolver {
        program,
        mut diagnostics,
        ..
    } = resolver;
    // Reorder by module, and by position within a file; ties keep discovery order.
    let order: FxHashMap<_, _> = modules
        .iter()
        .enumerate()
        .map(|(i, (_, m))| (m.file.clone(), i))
        .collect();
    diagnostics.sort_by_key(|d| {
        let file = order.get(&d.file).copied().unwrap_or(usize::MAX);
        (file, d.range.map(|r| (r.start(), r.end())))
    });
    (program, diagnostics)
}

/// For the import graph.
pub fn imports(file: &ast::SourceFile) -> Vec<Import> {
    file.imports()
        .filter_map(|import| {
            let path = import.path()?;
            let segments: Vec<SmolStr> = path.segments().map(|t| t.text().into()).collect();
            if segments.is_empty() {
                return None;
            }
            Some(Import {
                path: ModuleName::new(segments),
                range: path.syntax().text_range(),
            })
        })
        .collect()
}

pub(crate) fn token_name(token: &SyntaxToken) -> (SmolStr, TextRange) {
    (token.text().into(), token.text_range())
}

impl<'a> Resolver<'a> {
    fn new(modules: &'a ModuleMap, builtins: &'a dyn BuiltinModules) -> Self {
        let mut program = Program::default();
        build_prelude(&mut program);
        let mut prelude: [FxHashMap<SmolStr, DefId>; 3] = Default::default();
        for &def in &program.prelude.defs {
            let data = &program.defs[def];
            let ns = match data.kind {
                DefKind::Ctor => Ns::Ctor,
                DefKind::Fn => Ns::Value,
                _ => Ns::Type,
            };
            prelude[ns as usize].insert(data.name.clone(), def);
        }
        let mut members: FxHashMap<DefId, Vec<DefId>> = FxHashMap::default();
        for (def, data) in program.defs.iter() {
            if data.kind == DefKind::TraitFn
                && let Some(parent) = data.parent
            {
                members.entry(parent).or_default().push(def);
            }
        }
        Resolver {
            modules,
            builtins,
            builtin_modules: builtins.modules(),
            builtin_defs: FxHashMap::default(),
            program,
            scopes: ArenaMap::default(),
            prelude,
            members,
            labels: FxHashSet::default(),
            diagnostics: Vec::new(),
        }
    }

    pub(crate) fn error(&mut self, module: ModuleId, range: TextRange, kind: DiagnosticKind) {
        self.diagnostics.push(Diagnostic {
            file: self.modules[module].file.clone(),
            range: Some(range),
            kind,
        });
    }

    /// Records into [`Program::refs`].
    pub(crate) fn refer(&mut self, module: ModuleId, range: TextRange, target: RefTarget) {
        let span = Span { module, range };
        self.program.refs.push(NameRef { span, target });
    }

    pub(crate) fn alloc_def(
        &mut self,
        module: ModuleId,
        name: SmolStr,
        range: TextRange,
        kind: DefKind,
        parent: Option<DefId>,
    ) -> DefId {
        let (is_pub, is_opaque) = match parent {
            Some(p) if kind == DefKind::Ctor => {
                (self.program.defs[p].is_pub, self.program.defs[p].is_opaque)
            }
            _ => (false, false),
        };
        self.program.defs.alloc(DefData {
            name,
            kind,
            module: DefModule::Source(module),
            parent,
            span: Some(Span { module, range }),
            is_pub,
            is_opaque,
        })
    }

    /// If the namespace already has the name, reports a duplicate and keeps the earlier one.
    fn insert(&mut self, module: ModuleId, ns: Ns, name: SmolStr, entry: Entry<DefId>) {
        let scope = &mut self.scopes[module];
        match scope.names[ns as usize].get(&name) {
            Some(first) => {
                // Re-importing the same definition through another import is not a duplicate.
                if first.target == entry.target {
                    return;
                }
                let first = first.range;
                if let Some(range) = entry.range {
                    self.error(module, range, DiagnosticKind::Duplicate { name, first });
                }
            }
            None => {
                scope.names[ns as usize].insert(name, entry);
            }
        }
    }

    fn collect(&mut self, module: ModuleId, file: &ast::SourceFile) -> Vec<Pending> {
        let mut pending = Vec::new();
        let mut impl_count = 0;
        for item in file.items() {
            let syntax = item.syntax().clone();
            let (kind, name_token, is_pub, is_opaque) = match &item {
                ast::Item::Import(_) => continue,
                ast::Item::Fn(it) => (DefKind::Fn, it.name(), it.is_pub(), false),
                ast::Item::Type(it) => (DefKind::Type, it.name(), it.is_pub(), it.is_opaque()),
                ast::Item::Enum(it) => (DefKind::Enum, it.name(), it.is_pub(), it.is_opaque()),
                ast::Item::Error(it) => (DefKind::Error, it.name(), it.is_pub(), false),
                ast::Item::Const(it) => (DefKind::Const, it.name(), it.is_pub(), false),
                ast::Item::Effect(it) => (DefKind::Effect, it.name(), it.is_pub(), false),
                ast::Item::Handler(it) => (DefKind::Handler, it.name(), it.is_pub(), false),
                ast::Item::Layer(it) => (DefKind::Layer, it.name(), it.is_pub(), false),
                ast::Item::Trait(it) => (DefKind::Trait, it.name(), it.is_pub(), false),
                ast::Item::Impl(_) => (DefKind::Impl, None, false, false),
            };
            if !matches!(item, ast::Item::Fn(_)) {
                // A built-in effect (`Scope`, spec 11.2) and a built-in type (`Bytes`,
                // `[api.std.bytes]`) are marked with `@intrinsic`.
                let allowed = match &item {
                    ast::Item::Effect(_) => true,
                    ast::Item::Type(it) => it.field_list().is_none(),
                    _ => false,
                };
                self.check_intrinsic(module, &syntax, allowed);
            }
            let (name, range) = match &name_token {
                Some(token) => token_name(token),
                None if kind == DefKind::Impl => {
                    impl_count += 1;
                    (format!("impl#{impl_count}").into(), syntax.text_range())
                }
                // A declaration whose name could not be read already has a syntax diagnostic.
                None => continue,
            };
            let def = self.alloc_def(module, name.clone(), range, kind, None);
            self.program.defs[def].is_pub = is_pub;
            self.program.defs[def].is_opaque = is_opaque;
            self.program.modules[module].items.push(def);
            let entry = Entry {
                target: def,
                range: Some(range),
                imported: false,
            };
            match kind {
                DefKind::Fn | DefKind::Const => self.insert(module, Ns::Value, name.clone(), entry),
                DefKind::Error => {
                    self.insert(module, Ns::Type, name.clone(), entry.clone());
                    self.insert(module, Ns::Ctor, name.clone(), entry);
                }
                DefKind::Impl => {}
                _ => self.insert(module, Ns::Type, name.clone(), entry),
            }

            let mut children = Vec::new();
            let mut ctor = None;
            match &item {
                ast::Item::Type(it) if it.field_list().is_some() => {
                    ctor = Some(self.define_ctor(module, def, &name, range));
                }
                // A handler without fields still needs a constructor for its value if it has
                // `init` (spec 8.4).
                ast::Item::Handler(it) if it.field_list().is_some() || it.init().is_some() => {
                    ctor = Some(self.define_ctor(module, def, &name, range));
                }
                ast::Item::Enum(it) => {
                    for variant in it.variants() {
                        let Some(token) = variant.name() else {
                            continue;
                        };
                        let (vname, vrange) = token_name(&token);
                        let v =
                            self.alloc_def(module, vname.clone(), vrange, DefKind::Ctor, Some(def));
                        self.insert(
                            module,
                            Ns::Ctor,
                            vname,
                            Entry {
                                target: v,
                                range: Some(vrange),
                                imported: false,
                            },
                        );
                        children.push(v);
                    }
                }
                ast::Item::Effect(it) => {
                    if it.label().is_some() {
                        self.labels.insert(def);
                    }
                    let names = it.ops().filter_map(|op| op.name());
                    children = self.define_members(module, def, DefKind::Op, names);
                }
                ast::Item::Trait(it) => {
                    let names = it.fns().filter_map(|f| f.name());
                    children = self.define_members(module, def, DefKind::TraitFn, names);
                }
                _ => {}
            }
            if matches!(kind, DefKind::Effect | DefKind::Trait) {
                self.members.insert(def, children.clone());
            }
            pending.push(Pending {
                module,
                def,
                item,
                children,
                ctor,
            });
        }
        pending
    }

    /// `@intrinsic` is allowed only on fns without a body, on effects (built-in effects such
    /// as `Scope`, spec 8.1) and on types without fields (built-in types such as `Bytes`,
    /// `[kw.annotation]`) in the bundled core sources. `allowed` tells whether `node` is one of
    /// those. Fns in traits, impls and handlers are checked too (never allowed), although the
    /// parser does not put annotations inside those bodies yet.
    pub(crate) fn check_intrinsic(&mut self, module: ModuleId, node: &SyntaxNode, allowed: bool) {
        let Some(annotation) = node
            .children()
            .filter_map(ast::Annotation::cast)
            .find(|a| a.name().is_some_and(|n| n.text() == "intrinsic"))
        else {
            return;
        };
        let range = annotation.syntax().text_range();
        if !allowed {
            self.error(module, range, DiagnosticKind::MisplacedIntrinsic);
        } else if !self.modules.is_core(module) {
            self.error(module, range, DiagnosticKind::IntrinsicOutsideCore);
        }
    }

    /// The same-named constructor of a type or handler.
    fn define_ctor(
        &mut self,
        module: ModuleId,
        owner: DefId,
        name: &SmolStr,
        range: TextRange,
    ) -> DefId {
        let ctor = self.alloc_def(module, name.clone(), range, DefKind::Ctor, Some(owner));
        self.insert(
            module,
            Ns::Ctor,
            name.clone(),
            Entry {
                target: ctor,
                range: Some(range),
                imported: false,
            },
        );
        ctor
    }

    /// Effect operations and trait functions. Names may not repeat within a declaration.
    fn define_members(
        &mut self,
        module: ModuleId,
        parent: DefId,
        kind: DefKind,
        names: impl Iterator<Item = SyntaxToken>,
    ) -> Vec<DefId> {
        let mut seen: FxHashMap<SmolStr, TextRange> = FxHashMap::default();
        let mut defs = Vec::new();
        for token in names {
            let (name, range) = token_name(&token);
            if let Some(&first) = seen.get(&name) {
                self.error(
                    module,
                    range,
                    DiagnosticKind::Duplicate {
                        name,
                        first: Some(first),
                    },
                );
                continue;
            }
            seen.insert(name.clone(), range);
            defs.push(self.alloc_def(module, name, range, kind, Some(parent)));
        }
        defs
    }

    fn imports(&mut self, module: ModuleId, file: &ast::SourceFile) {
        for import in file.imports() {
            let Some(path) = import.path() else { continue };
            let segments: Vec<SyntaxToken> = path.segments().collect();
            let Some(last) = segments.last() else {
                continue;
            };
            let name = ModuleName::new(segments.iter().map(|t| SmolStr::from(t.text())));
            // The import graph has already reported undefined modules.
            let Some(target) = self.modules.lookup_from(module, &name) else {
                continue;
            };
            // Each path prefix points to the module of that name (the `Http` of
            // `import Http.Client`).
            for (i, segment) in segments.iter().enumerate() {
                let prefix =
                    ModuleName::new(segments[..=i].iter().map(|t| SmolStr::from(t.text())));
                if let Some(m) = self.modules.lookup_from(module, &prefix) {
                    self.refer(module, segment.text_range(), RefTarget::Module(m));
                }
            }
            let (alias, alias_range) = token_name(last);
            let scope = &mut self.scopes[module];
            match scope.modules.get(&alias) {
                Some(first) if first.target != ModRef::Source(target) => {
                    let first = first.range;
                    self.error(
                        module,
                        alias_range,
                        DiagnosticKind::Duplicate { name: alias, first },
                    );
                }
                Some(_) => {}
                None => {
                    scope.modules.insert(
                        alias,
                        Entry {
                            target: ModRef::Source(target),
                            range: Some(alias_range),
                            imported: true,
                        },
                    );
                }
            }
            for token in import.names() {
                self.import_name(module, target, &name, &token);
            }
        }
    }

    /// The `x` of `import A.B.{x}`. Brings in the pub definitions from every namespace.
    fn import_name(
        &mut self,
        module: ModuleId,
        target: ModuleId,
        target_name: &ModuleName,
        token: &SyntaxToken,
    ) {
        let (name, range) = token_name(token);
        let found: Vec<(Ns, DefId)> = NAMESPACES
            .iter()
            .filter_map(|&ns| {
                let entry = self.scopes[target].get(ns, &name)?;
                (!entry.imported).then_some((ns, entry.target))
            })
            .collect();
        if found.is_empty() {
            let suggestion = best_match(&name, self.exported_names(target));
            self.error(
                module,
                range,
                DiagnosticKind::UndefinedMember {
                    name,
                    owner: target_name.to_string().into(),
                    owner_kind: "module",
                    suggestion,
                },
            );
            return;
        }
        let public: Vec<_> = found
            .into_iter()
            .filter(|&(_, def)| self.program.defs[def].is_pub)
            .collect();
        if public.is_empty() {
            self.error(
                module,
                range,
                DiagnosticKind::Private {
                    name,
                    module: target_name.clone(),
                },
            );
            return;
        }
        for (ns, def) in public {
            self.refer(module, range, RefTarget::Def(def));
            self.insert(
                module,
                ns,
                name.clone(),
                Entry {
                    target: def,
                    range: Some(range),
                    imported: true,
                },
            );
        }
    }

    fn exported_names(&self, module: ModuleId) -> Vec<&SmolStr> {
        let scope = &self.scopes[module];
        NAMESPACES
            .iter()
            .flat_map(|&ns| {
                scope.names[ns as usize]
                    .iter()
                    .filter(|(_, e)| !e.imported)
                    .map(|(n, _)| n)
            })
            .collect()
    }

    /// Warns if module definitions or imports hide Prelude names.
    fn warn_prelude_shadowing(&mut self, module: ModuleId) {
        let mut shadowing: Vec<(TextRange, SmolStr)> = Vec::new();
        for ns in NAMESPACES {
            for (name, entry) in &self.scopes[module].names[ns as usize] {
                if self.prelude[ns as usize].contains_key(name)
                    && let Some(range) = entry.range
                {
                    shadowing.push((range, name.clone()));
                }
            }
        }
        // A definition hiding both a type and a constructor (type, error) is reported once.
        shadowing.sort_by_key(|(range, _)| (range.start(), range.end()));
        shadowing.dedup();
        for (range, name) in shadowing {
            self.error(module, range, DiagnosticKind::ShadowsPrelude { name });
        }
    }

    /// Looks in the module's namespaces and the Prelude. Locals and type parameters are checked
    /// first by the caller.
    pub(crate) fn lookup(&self, module: ModuleId, ns: Ns, name: &str) -> Option<DefId> {
        self.scopes[module]
            .get(ns, name)
            .map(|e| e.target)
            .or_else(|| self.prelude[ns as usize].get(name).copied())
    }

    /// "Did you mean" candidates: names in the module's namespaces and the Prelude.
    pub(crate) fn candidates(&self, module: ModuleId, ns: Ns) -> Vec<SmolStr> {
        self.scopes[module]
            .names(ns)
            .chain(self.prelude[ns as usize].keys())
            .cloned()
            .collect()
    }

    /// Looks up a type name left of `.`. If not found at module level (definitions and imports),
    /// tries the built-in modules and the Prelude traits. If a module and a trait are both found
    /// at the same level, reports an ambiguity.
    pub(crate) fn lookup_dot_left(
        &mut self,
        module: ModuleId,
        name: &SmolStr,
        range: TextRange,
    ) -> Option<DotLeft> {
        let scope = &self.scopes[module];
        let as_module = scope.modules.get(name).map(|e| e.target.clone());
        let as_trait = scope
            .get(Ns::Type, name)
            .map(|e| e.target)
            .filter(|&d| self.program.defs[d].kind == DefKind::Trait);
        let found = match (as_module, as_trait) {
            (Some(_), Some(_)) => {
                self.error(
                    module,
                    range,
                    DiagnosticKind::Ambiguous { name: name.clone() },
                );
                return None;
            }
            (Some(m), None) => Some(DotLeft::Module(m)),
            (None, Some(t)) => Some(DotLeft::Trait(t)),
            (None, None) => {
                if let Some(core) = self.core_module(name) {
                    Some(DotLeft::Module(ModRef::Source(core)))
                } else if self.builtin_modules.contains(name) {
                    Some(DotLeft::Module(ModRef::Builtin(name.clone())))
                } else {
                    self.prelude[Ns::Type as usize]
                        .get(name)
                        .copied()
                        .filter(|&d| self.program.defs[d].kind == DefKind::Trait)
                        .map(DotLeft::Trait)
                }
            }
        };
        if found.is_some() {
            return found;
        }
        // If it is a name of another kind, say so.
        if let Some(def) = self.lookup(module, Ns::Type, name) {
            let found = self.program.defs[def].kind.describe();
            self.error(
                module,
                range,
                DiagnosticKind::WrongKind {
                    name: name.clone(),
                    expected: Expected::ModuleOrTrait,
                    found,
                },
            );
            return None;
        }
        let mut candidates: Vec<SmolStr> = self.scopes[module].modules.keys().cloned().collect();
        candidates.extend(self.builtin_modules.iter().cloned());
        candidates.extend(
            self.modules
                .iter()
                .filter(|&(id, _)| self.modules.is_core(id))
                .map(|(_, m)| m.name.last().clone()),
        );
        candidates.extend(self.candidates(module, Ns::Type).into_iter().filter(|n| {
            self.lookup(module, Ns::Type, n)
                .is_some_and(|d| self.program.defs[d].kind == DefKind::Trait)
        }));
        let suggestion = best_match(name, &candidates);
        self.error(
            module,
            range,
            DiagnosticKind::UndefinedName {
                name: name.clone(),
                expected: Expected::ModuleOrTrait,
                suggestion,
            },
        );
        None
    }

    /// A non-pub definition from another module is reported, then still returned so resolution
    /// can continue. Reports and returns `None` if not found.
    pub(crate) fn module_member(
        &mut self,
        module: ModuleId,
        target: &ModRef,
        ns: Ns,
        name: &SmolStr,
        range: TextRange,
    ) -> Option<DefId> {
        match target {
            ModRef::Source(target) => {
                let target = *target;
                let found = self.scopes[target]
                    .get(ns, name)
                    .filter(|e| !e.imported)
                    .map(|e| e.target);
                let module_name = self.modules[target].name.clone();
                match found {
                    Some(def) => {
                        if target != module && !self.program.defs[def].is_pub {
                            self.error(
                                module,
                                range,
                                DiagnosticKind::Private {
                                    name: name.clone(),
                                    module: module_name,
                                },
                            );
                        }
                        Some(def)
                    }
                    None => {
                        let candidates: Vec<SmolStr> =
                            self.scopes[target].names(ns).cloned().collect();
                        let suggestion = best_match(name, &candidates);
                        self.error(
                            module,
                            range,
                            DiagnosticKind::UndefinedMember {
                                name: name.clone(),
                                owner: module_name.to_string().into(),
                                owner_kind: "module",
                                suggestion,
                            },
                        );
                        None
                    }
                }
            }
            ModRef::Builtin(target) => {
                let kind = self.builtins.lookup(target, name);
                let fits = match (kind, ns) {
                    (Some(BuiltinKind::Fn), Ns::Value) => Some(DefKind::Fn),
                    (Some(BuiltinKind::Type), Ns::Type) => Some(DefKind::BuiltinType),
                    _ => None,
                };
                let Some(kind) = fits else {
                    let candidates = self.builtins.names(target);
                    let suggestion = best_match(name, &candidates);
                    self.error(
                        module,
                        range,
                        DiagnosticKind::UndefinedMember {
                            name: name.clone(),
                            owner: target.clone(),
                            owner_kind: "module",
                            suggestion,
                        },
                    );
                    return None;
                };
                let key = (target.clone(), name.clone());
                if let Some(&def) = self.builtin_defs.get(&key) {
                    return Some(def);
                }
                let def = self.program.defs.alloc(DefData {
                    name: name.clone(),
                    kind,
                    module: DefModule::Builtin(target.clone()),
                    parent: None,
                    span: None,
                    is_pub: true,
                    is_opaque: false,
                });
                self.builtin_defs.insert(key, def);
                Some(def)
            }
        }
    }

    /// If a bundled core module or built-in module `X` has a type `X` of the same name, returns
    /// it (`Map[K, V]`, spec 5.7, 4.6). Used in type positions when the name is not a type
    /// parameter, a module definition or import, or a Prelude name.
    pub(crate) fn builtin_eponymous_type(
        &mut self,
        module: ModuleId,
        name: &SmolStr,
    ) -> Option<DefId> {
        if let Some(core) = self.core_module(name) {
            return self.scopes[core]
                .get(Ns::Type, name)
                .filter(|e| !e.imported && self.program.defs[e.target].is_pub)
                .map(|e| e.target);
        }
        if !self.builtin_modules.contains(name)
            || self.builtins.lookup(name, name) != Some(BuiltinKind::Type)
        {
            return None;
        }
        let target = ModRef::Builtin(name.clone());
        self.module_member(module, &target, Ns::Type, name, TextRange::default())
    }

    /// A single-segment bundled core module. Can be written left of `.` without an import
    /// (spec 4.6).
    fn core_module(&self, name: &SmolStr) -> Option<ModuleId> {
        self.modules
            .lookup(&ModuleName::new([name.clone()]))
            .filter(|&id| self.modules.is_core(id))
    }

    /// If the bundled core has a `Prelude`, its pub definitions override the Prelude names.
    /// Names it does not define stay as in the built-in Prelude ([`build_prelude`]), so the
    /// Prelude can move to the bundled `prelude.emel` a name at a time.
    fn use_core_prelude(&mut self) {
        let Some(prelude) = self.core_module(&SmolStr::new(crate::PRELUDE)) else {
            return;
        };
        for ns in NAMESPACES {
            let defs: Vec<(SmolStr, DefId)> = self.scopes[prelude].names[ns as usize]
                .iter()
                .filter(|(_, e)| !e.imported && self.program.defs[e.target].is_pub)
                .map(|(n, e)| (n.clone(), e.target))
                .collect();
            for (name, def) in defs {
                if let Some(old) = self.prelude[ns as usize].insert(name, def) {
                    self.program.prelude.defs.retain(|&d| d != old);
                }
                if !self.program.prelude.defs.contains(&def) {
                    self.program.prelude.defs.push(def);
                }
            }
        }
    }

    pub(crate) fn member(&self, owner: DefId, name: &str) -> Option<DefId> {
        self.members
            .get(&owner)?
            .iter()
            .copied()
            .find(|&d| self.program.defs[d].name == name)
    }

    pub(crate) fn member_names(&self, owner: DefId) -> Vec<SmolStr> {
        self.members
            .get(&owner)
            .into_iter()
            .flatten()
            .map(|&d| self.program.defs[d].name.clone())
            .collect()
    }

    /// For diagnostic messages.
    pub(crate) fn def_module_name(&self, def: DefId) -> ModuleName {
        match &self.program.defs[def].module {
            DefModule::Source(m) => self.modules[*m].name.clone(),
            DefModule::Prelude => ModuleName::new([crate::PRELUDE]),
            DefModule::Builtin(name) => ModuleName::new([name.clone()]),
        }
    }
}

pub(crate) fn child_token(
    node: &SyntaxNode,
    kind: emela_syntax::SyntaxKind,
) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == kind)
}
