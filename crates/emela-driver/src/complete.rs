//! Completion (LSP `textDocument/completion`).
//!
//! Source being edited is syntactically broken, so the context (inside an import, after a `.`)
//! is decided from the text before the cursor, and candidates come from the resolver and type
//! checker results.

use emela_resolve::hir::{
    DefId, DefKind, Expr, ExprId, ExprKind, Fields, Item, LocalId, LocalKind, PatId, PatKind,
    Program, Res, Stmt,
};
use emela_resolve::{ModuleId, ModuleName};
use emela_syntax::ast::AstNode;
use emela_types::{Ty, TyParam};
use line_index::{TextRange, TextSize};

use crate::frontend::TypedProgram;
use crate::pipeline::Analysis;
use crate::source::FileId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub label: String,
    pub kind: CompletionKind,
    /// A description shown beside the candidate, such as a type or module name.
    pub detail: Option<String>,
    /// Text inserted when chosen; `None` means `label`.
    pub insert_text: Option<String>,
    /// The definition's `##` comment, as Markdown.
    pub documentation: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompletionKind {
    /// A local (parameter, binding, pattern variable).
    Local,
    Fn,
    Const,
    /// A constructor (a type's constructor, an enum variant).
    Ctor,
    Type,
    Enum,
    Error,
    Effect,
    Handler,
    Layer,
    Trait,
    /// An effect operation or a trait function.
    Method,
    Field,
    Module,
    Keyword,
    /// A named argument label (`h: `).
    Label,
}

/// Keywords (spec 3.1) offered at the start of a word. `as` has no syntax of its own, so it is
/// not offered.
const KEYWORDS: &[&str] = &[
    "fn", "type", "enum", "error", "const", "trait", "impl", "effect", "handler", "layer",
    "import", "pub", "opaque", "suspend", "if", "else", "match", "fail", "escape", "use", "with",
    "assert", "self", "Self",
];

/// Filtering by the partially typed name is left to the editor.
pub(crate) fn complete(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    offset: TextSize,
) -> Vec<Completion> {
    let Some(module) = module_of(analysis, file) else {
        return Vec::new();
    };
    let text = analysis.sources[file].text();
    let Some(before) = text.get(..usize::from(offset)) else {
        return Vec::new();
    };
    let cx = Context {
        program,
        analysis,
        module,
        text,
        docs: crate::documentation::Documentation::default(),
    };
    // Before the partially typed name.
    let start = before.trim_end_matches(is_name_char).len();
    let line = &before[before[..start].rfind('\n').map_or(0, |i| i + 1)..start];
    let mut items = if let Some(rest) = line.trim_start().strip_prefix("import ") {
        cx.import(rest)
    } else if before[..start].ends_with('.') {
        cx.after_dot(&before[..start - 1])
    } else {
        match before[..start]
            .trim_end()
            .rsplit(|c: char| !is_name_char(c))
            .next()
        {
            Some("use") => cx.in_scope(offset, |kind| kind == DefKind::Effect),
            Some("with") => cx.in_scope(offset, |kind| {
                matches!(kind, DefKind::Handler | DefKind::Layer)
            }),
            _ => {
                // At the start of a call argument, offer labels not yet written first.
                let mut items = labels(program, analysis, file, offset, start, &cx.docs);
                items.extend(cx.in_scope(offset, |_| true));
                items.extend(KEYWORDS.iter().map(|k| Completion {
                    label: (*k).to_owned(),
                    kind: CompletionKind::Keyword,
                    documentation: None,
                    detail: None,
                    insert_text: None,
                }));
                items
            }
        }
    };
    // For duplicate names, keep the first found (the innermost).
    let mut seen = std::collections::HashSet::new();
    items.retain(|item| seen.insert(item.label.clone()));
    items
}

/// At the start of a call argument (after `f(` or a comma, with only a partial name), the
/// callee's labels not yet written. Choosing one inserts `label: `; the detail is the parameter
/// type.
fn labels(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    offset: TextSize,
    start: usize,
    docs: &crate::documentation::Documentation,
) -> Vec<Completion> {
    let text = analysis.sources[file].text();
    let Some(call) = crate::signature::call_at(program, analysis, file, offset) else {
        return Vec::new();
    };
    let Some(args) = text.get(usize::from(call.open) + 1..start) else {
        return Vec::new();
    };
    let parts = crate::signature::split_args(args);
    let (current, written) = parts.split_last().expect("there is at least one part");
    if !current.trim().is_empty() {
        return Vec::new();
    }
    let written: Vec<&str> = written
        .iter()
        .filter_map(|a| crate::signature::arg_label(a))
        .collect();
    let Some((_, Ty::Fn(mut f), params, labels)) =
        crate::signature::callee_type(program, call.callee)
    else {
        return Vec::new();
    };
    // On the right of a pipe the first argument is filled by the left side, so its label is not
    // offered.
    let labels = crate::signature::without_piped(call.piped, &mut f.params, labels);
    let cons = &program.types.ty_table.cons;
    let callee = crate::signature::callee_def(program, call.callee);
    labels
        .iter()
        .zip(&f.params)
        .filter_map(|(label, ty)| {
            let label = label.as_deref()?;
            (!written.contains(&label)).then(|| Completion {
                label: format!("{label}:"),
                kind: CompletionKind::Label,
                documentation: callee.and_then(|def| docs.field(program, analysis, def, label)),
                detail: Some(ty.display_with(cons, &params).to_string()),
                insert_text: Some(format!("{label}: ")),
            })
        })
        .collect()
}

struct Context<'a> {
    program: &'a TypedProgram,
    analysis: &'a Analysis,
    module: ModuleId,
    text: &'a str,
    docs: crate::documentation::Documentation,
}

impl Context<'_> {
    fn hir(&self) -> &Program {
        &self.program.hir
    }

    /// After `import`: inside `{`, the module's pub names; before it, the next segment of the
    /// module name.
    fn import(&self, rest: &str) -> Vec<Completion> {
        if let Some((path, _)) = rest.split_once('{') {
            let name = ModuleName::parse(path.trim().trim_end_matches('.'));
            let Some(target) = self.analysis.modules.lookup(&name) else {
                return Vec::new();
            };
            return self.members(target);
        }
        // For `import Http.`, the next segment of module names starting with `Http.`.
        let typed: Vec<&str> = rest.trim().split('.').collect();
        let typed = &typed[..typed.len() - 1];
        let mut items: Vec<Completion> = self
            .analysis
            .modules
            .iter()
            .filter(|&(id, _)| id != self.module)
            .filter_map(|(_, data)| {
                let segments = data.name.segments();
                if segments.len() <= typed.len() || segments.iter().zip(typed).any(|(s, t)| s != t)
                {
                    return None;
                }
                Some(Completion {
                    label: segments[typed.len()].to_string(),
                    kind: CompletionKind::Module,
                    documentation: None,
                    detail: Some(data.name.to_string()),
                    insert_text: None,
                })
            })
            .collect();
        items.sort_by(|a, b| a.label.cmp(&b.label));
        items
    }

    /// After `.`: if the left is a run of uppercase names, the contents of a module or trait;
    /// otherwise, from the expression's type, operations of a capability value, or fields and
    /// methods.
    fn after_dot(&self, left: &str) -> Vec<Completion> {
        let path = &left[left.trim_end_matches(|c| is_name_char(c) || c == '.').len()..];
        let segments: Vec<&str> = path.split('.').collect();
        if segments
            .iter()
            .all(|s| s.chars().next().is_some_and(|c| c.is_ascii_uppercase()))
        {
            return match self.dot_left(&segments) {
                Some(Left::Module(m)) => self.members(m),
                Some(Left::Trait(t)) => self.trait_fns(t),
                None => Vec::new(),
            };
        }
        self.expr_members(TextSize::of(left))
    }

    /// The name left of `.` (spec 4.6): an imported name or core module, otherwise a trait. From
    /// the second segment on, pub traits of the module.
    fn dot_left(&self, segments: &[&str]) -> Option<Left> {
        let (first, rest) = segments.split_first()?;
        let mut left = if let Some(m) = self.imported_module(first) {
            Left::Module(m)
        } else if let Some(m) = self
            .analysis
            .modules
            .lookup(&ModuleName::new([*first]))
            .filter(|&m| self.analysis.modules.is_core(m))
        {
            Left::Module(m)
        } else {
            Left::Trait(self.trait_named(self.module, first, false)?)
        };
        for segment in rest {
            let Left::Module(m) = left else { return None };
            left = Left::Trait(self.trait_named(m, segment, true)?);
        }
        Some(left)
    }

    /// The module `B` brought in by this file's `import A.B`.
    fn imported_module(&self, alias: &str) -> Option<ModuleId> {
        imports(self.text)
            .into_iter()
            .find(|(path, _)| path.last() == alias)
            .and_then(|(path, _)| self.analysis.modules.lookup(&path))
    }

    /// A trait of the module or of the Prelude.
    fn trait_named(&self, module: ModuleId, name: &str, public: bool) -> Option<DefId> {
        let hir = self.hir();
        let own = hir.modules.get(module)?.items.iter().copied();
        let prelude = hir.prelude.defs.iter().copied().filter(|_| !public);
        own.chain(prelude).find(|&def| {
            let data = &hir.defs[def];
            data.kind == DefKind::Trait && data.name == name && (!public || data.is_pub)
        })
    }

    /// pub definitions of the module (names other modules can reach), including enum variants.
    fn members(&self, module: ModuleId) -> Vec<Completion> {
        let hir = self.hir();
        let Some(items) = hir.modules.get(module) else {
            return Vec::new();
        };
        let mut defs = Vec::new();
        for &def in &items.items {
            defs.push(def);
            if let Some(Item::Enum(e)) = hir.item(def) {
                defs.extend(e.variants.iter().copied());
            }
        }
        defs.into_iter()
            .filter(|&def| hir.defs[def].is_pub)
            .filter_map(|def| self.def_item(def))
            .collect()
    }

    fn trait_fns(&self, def: DefId) -> Vec<Completion> {
        match self.hir().item(def) {
            Some(Item::Trait(t)) => t.fns.iter().filter_map(|&f| self.def_item(f)).collect(),
            _ => Vec::new(),
        }
    }

    /// From the type of the expression ending right before `.` (`[lsp.completion]`): operations
    /// of a capability value, or fields of a type or error followed by the functions that can be
    /// called as methods.
    fn expr_members(&self, end: TextSize) -> Vec<Completion> {
        let hir = self.hir();
        let types = &self.program.types;
        // The outermost expression ending at that position (`a.b` for `a.b.`).
        let Some((expr, _)) = hir
            .exprs
            .iter()
            .filter(|(_, e)| e.span.module == self.module && e.span.range.end() == end)
            .filter(|(_, e)| !matches!(e.kind, ExprKind::Missing))
            .max_by_key(|(_, e)| e.span.range.len())
        else {
            return Vec::new();
        };
        let Some(ty) = self.expr_ty(expr) else {
            return Vec::new();
        };
        let mut items = self.fields_of(&ty);
        // Then the functions that can be called as methods (`[type.rules]`).
        let env = self.enclosing_params(end);
        let cons = &types.ty_table.cons;
        for def in emela_types::methods_of(hir, types, self.module, &ty, &env) {
            let Some(sig) = types.sigs.get(def) else {
                continue;
            };
            let Ty::Fn(f) = &sig.scheme.ty else {
                continue;
            };
            // The first parameter is x of `x.f(...)`.
            let called = Ty::func_sets(
                f.params.iter().skip(1).cloned(),
                f.ret.clone(),
                f.fails.clone(),
                f.uses.clone(),
            );
            items.push(Completion {
                label: hir.defs[def].name.to_string(),
                kind: CompletionKind::Method,
                detail: Some(called.display_with(cons, &sig.scheme.params).to_string()),
                documentation: self.docs.definition(self.program, self.analysis, def),
                insert_text: None,
            });
        }
        items
    }

    /// The operations of a capability value, or the fields of a type, an error or a handler.
    fn fields_of(&self, ty: &Ty) -> Vec<Completion> {
        let hir = self.hir();
        let types = &self.program.types;
        match ty {
            Ty::Cap(effect) => hir
                .effect_ops(effect.def)
                .iter()
                .filter_map(|&op| self.def_item(op))
                .collect(),
            Ty::Named(con, _) => {
                let def = types.ty_table.def(*con);
                let fields = match hir.item(def) {
                    Some(Item::Type(t)) => match &t.fields {
                        Fields::Named(fields) => fields,
                        _ => return Vec::new(),
                    },
                    Some(Item::Error(e)) => match &e.fields {
                        Fields::Named(fields) => fields,
                        _ => return Vec::new(),
                    },
                    // A handler value (`self.`, spec 8.4).
                    Some(Item::Handler(h)) => &h.fields,
                    _ => return Vec::new(),
                };
                fields
                    .iter()
                    .map(|f| Completion {
                        label: f.name.to_string(),
                        kind: CompletionKind::Field,
                        documentation: self.docs.field(self.program, self.analysis, def, &f.name),
                        detail: hir.types[f.ty].span.and_then(|s| self.source_text(s)),
                        insert_text: None,
                    })
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// The expression's type. If it is not in the type table (for `log.` or `a.b.`, the nameless
    /// `.` is `Missing` in the resolver and the left expression never reaches inference), it is
    /// computed from the left: locals and definitions use their declared types, `x.f` the field
    /// type, the capability operation's type or the method's type, and calls the return type of
    /// the callee.
    fn expr_ty(&self, expr: ExprId) -> Option<Ty> {
        let hir = self.hir();
        let types = &self.program.types;
        if let Some(ty) = types.exprs.get(expr) {
            return Some(ty.clone());
        }
        match &hir.exprs[expr].kind {
            ExprKind::Path(Res::Local(local)) => types.locals.get(*local).cloned(),
            ExprKind::Path(Res::Def(def)) => types.sigs.get(*def).map(|sig| sig.scheme.ty.clone()),
            ExprKind::Field { base, name } => match self.expr_ty(*base)? {
                Ty::Cap(effect) => {
                    let op = hir
                        .effect_ops(effect.def)
                        .iter()
                        .find(|&&op| hir.defs[op].name == name.name)?;
                    // Effect type parameters are replaced by the capability value's type arguments
                    // (spec 8.1).
                    types
                        .sigs
                        .get(*op)
                        .map(|sig| emela_types::substitute_bound(&sig.scheme.ty, &effect.args))
                }
                base_ty => {
                    let field = match &base_ty {
                        Ty::Named(con, args) => {
                            self.field_ty(types.ty_table.def(*con), args, &name.name)
                        }
                        _ => None,
                    };
                    field.or_else(|| self.method_ty(&base_ty, &name.name, expr))
                }
            },
            ExprKind::Call { callee, .. } => match self.expr_ty(*callee)? {
                Ty::Fn(f) => Some(f.ret),
                _ => None,
            },
            _ => None,
        }
    }

    /// The declared type of the function `name` that a method call on a value of type `ty` calls
    /// (`[type.rules]`), for the left of `xs.map(f).`. Its type parameters are not instantiated:
    /// only the head of its return type is used.
    fn method_ty(&self, ty: &Ty, name: &str, expr: ExprId) -> Option<Ty> {
        let hir = self.hir();
        let types = &self.program.types;
        let env = self.enclosing_params(hir.exprs[expr].span.range.end());
        emela_types::methods_of(hir, types, self.module, ty, &env)
            .into_iter()
            .find(|&def| hir.defs[def].name == name)
            .and_then(|def| types.sigs.get(def))
            .map(|sig| sig.scheme.ty.clone())
    }

    /// The type of a named field of a type or error. Taken from the constructor's signature
    /// (parameters are field types), with type parameters replaced by `args`.
    fn field_ty(&self, def: DefId, args: &[Ty], name: &str) -> Option<Ty> {
        let hir = self.hir();
        let ctor = match hir.item(def)? {
            Item::Type(t) => t.ctor?,
            Item::Error(_) => def,
            Item::Handler(h) => h.ctor?,
            _ => return None,
        };
        let sig = self.program.types.sigs.get(ctor)?;
        let index = sig.labels.iter().position(|l| l.as_deref() == Some(name))?;
        let Ty::Fn(f) = &sig.scheme.ty else {
            return None;
        };
        Some(emela_types::substitute_bound(f.params.get(index)?, args))
    }

    /// Names visible from the position: locals (innermost first), this module's definitions,
    /// imported names and modules, core modules, and the Prelude, in that order. `want` filters
    /// definitions by kind (locals and modules are offered only when nothing is filtered).
    fn in_scope(&self, offset: TextSize, want: impl Fn(DefKind) -> bool) -> Vec<Completion> {
        let hir = self.hir();
        let all = want(DefKind::Fn) && want(DefKind::Type);
        let mut items = Vec::new();
        if all {
            let params = self.enclosing_params(offset);
            for local in visible_locals(hir, self.module, offset) {
                let data = &hir.locals[local];
                let detail = self.program.types.locals.get(local).map(|ty| {
                    ty.display_with(&self.program.types.ty_table.cons, &params)
                        .to_string()
                });
                items.push(Completion {
                    label: data.name.to_string(),
                    kind: CompletionKind::Local,
                    documentation: None,
                    detail,
                    insert_text: None,
                });
            }
        }
        let mut defs: Vec<DefId> = Vec::new();
        if let Some(own) = hir.modules.get(self.module) {
            for &def in &own.items {
                defs.push(def);
                if let Some(Item::Enum(e)) = hir.item(def) {
                    defs.extend(e.variants.iter().copied());
                }
            }
        }
        let imports = imports(self.text);
        for (path, names) in &imports {
            let Some(target) = self.analysis.modules.lookup(path) else {
                continue;
            };
            let Some(module) = hir.modules.get(target) else {
                continue;
            };
            let mut found: Vec<DefId> = module.items.clone();
            for &def in &module.items {
                if let Some(Item::Enum(e)) = hir.item(def) {
                    found.extend(e.variants.iter().copied());
                }
            }
            defs.extend(found.into_iter().filter(|&def| {
                let data = &hir.defs[def];
                data.is_pub && names.iter().any(|n| *n == data.name)
            }));
        }
        defs.extend(hir.prelude.defs.iter().copied());
        items.extend(
            defs.into_iter()
                .filter(|&def| want(hir.defs[def].kind))
                .filter_map(|def| self.def_item(def)),
        );
        if all {
            // Imported modules, and core modules that can be written left of `.` without an
            // import.
            let modules = imports.iter().map(|(path, _)| path.clone()).chain(
                self.analysis
                    .modules
                    .iter()
                    .filter(|&(id, data)| {
                        self.analysis.modules.is_core(id)
                            && data.name.segments().len() == 1
                            && data.name.last() != emela_resolve::PRELUDE
                    })
                    .map(|(_, data)| data.name.clone()),
            );
            for name in modules {
                items.push(Completion {
                    label: name.last().to_string(),
                    kind: CompletionKind::Module,
                    documentation: None,
                    detail: Some(name.to_string()),
                    insert_text: None,
                });
            }
        }
        items
    }

    /// Definitions without a kind (impls and their functions, type parameters) are not offered. A constructor named
    /// like its type is offered as the type (they merge later since the names match).
    fn def_item(&self, def: DefId) -> Option<Completion> {
        let data = &self.hir().defs[def];
        let kind = match data.kind {
            DefKind::Fn => CompletionKind::Fn,
            DefKind::Const => CompletionKind::Const,
            DefKind::Ctor => CompletionKind::Ctor,
            DefKind::Type | DefKind::BuiltinType => CompletionKind::Type,
            DefKind::Enum => CompletionKind::Enum,
            DefKind::Error => CompletionKind::Error,
            DefKind::Effect => CompletionKind::Effect,
            DefKind::Handler => CompletionKind::Handler,
            DefKind::Layer => CompletionKind::Layer,
            DefKind::Trait => CompletionKind::Trait,
            DefKind::Op | DefKind::TraitFn => CompletionKind::Method,
            DefKind::Impl | DefKind::ImplFn | DefKind::TypeParam => return None,
        };
        let types = &self.program.types;
        let detail = types
            .sigs
            .get(def)
            .map(|sig| sig.scheme.display(&types.ty_table.cons).to_string());
        Some(Completion {
            label: data.name.to_string(),
            kind,
            detail,
            documentation: self.docs.definition(self.program, self.analysis, def),
            insert_text: None,
        })
    }

    /// Type parameters of the definition whose body contains the position (names of `Bound(i)`
    /// in local types).
    fn enclosing_params(&self, offset: TextSize) -> Vec<TyParam> {
        crate::hover::enclosing_def(self.program, self.module, offset)
            .and_then(|def| self.program.types.sigs.get(def))
            .map(|sig| sig.scheme.params.clone())
            .unwrap_or_default()
    }

    fn source_text(&self, span: emela_resolve::hir::Span) -> Option<String> {
        let file = *self.analysis.files.get(span.module)?;
        let text = self.analysis.sources[file].text();
        text.get(std::ops::Range::<usize>::from(span.range))
            .map(str::to_owned)
    }
}

enum Left {
    Module(ModuleId),
    Trait(DefId),
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn module_of(analysis: &Analysis, file: FileId) -> Option<ModuleId> {
    analysis
        .files
        .iter()
        .find(|(_, f)| **f == file)
        .map(|(m, _)| m)
}

/// The file's imports: module names and the names listed in braces.
fn imports(text: &str) -> Vec<(ModuleName, Vec<String>)> {
    emela_syntax::parse(text)
        .tree()
        .imports()
        .filter_map(|import| {
            let path = import.path()?;
            let name = ModuleName::new(path.segments().map(|t| t.text().to_owned()));
            if name.segments().is_empty() || path.syntax().text_range().is_empty() {
                return None;
            }
            Some((name, import.names().map(|t| t.text().to_owned()).collect()))
        })
        .collect()
}

/// Locals visible from the position, innermost (bound later) first. Only the innermost of each
/// name.
fn visible_locals(
    hir: &Program,
    module: emela_resolve::ModuleId,
    offset: TextSize,
) -> Vec<LocalId> {
    let mut scopes: Vec<(LocalId, TextRange)> = Vec::new();
    let mut add = |locals: Vec<LocalId>, range: TextRange| {
        scopes.extend(locals.into_iter().map(|l| (l, range)));
    };
    let span_of = |expr: emela_resolve::hir::ExprId| hir.exprs[expr].span.range;
    for (_, item) in hir.items.iter() {
        match item {
            Item::Fn(f) | Item::TraitFn(f) => {
                if let Some(body) = f.body {
                    add(
                        f.sig.params.iter().map(|p| p.local).collect(),
                        span_of(body),
                    );
                }
            }
            Item::Handler(h) => {
                for op in &h.ops {
                    if let Some(body) = op.body {
                        let mut locals = vec![op.self_local];
                        locals.extend(op.params.iter().map(|p| p.local));
                        add(locals, span_of(body));
                    }
                }
                if let Some(body) = h.init {
                    add(
                        h.init_params.iter().map(|p| p.local).collect(),
                        span_of(body),
                    );
                }
                if let Some((local, body)) = h.release {
                    add(vec![local], span_of(body));
                }
            }
            Item::Impl(i) => {
                for f in &i.fns {
                    if let Some(body) = f.body {
                        add(f.params.iter().map(|p| p.local).collect(), span_of(body));
                    }
                }
            }
            _ => {}
        }
    }
    for (_, Expr { kind, span }) in hir.exprs.iter() {
        if span.module != module || !span.range.contains_inclusive(offset) {
            continue;
        }
        match kind {
            ExprKind::Block(block) => {
                for stmt in &block.stmts {
                    if let Stmt::Let { pat, value, .. } = stmt {
                        // From after the right-hand side to the end of the block (spec 6.3).
                        let start = span_of(*value).end();
                        if start <= span.range.end() {
                            add(
                                pat_locals(hir, *pat),
                                TextRange::new(start, span.range.end()),
                            );
                        }
                    }
                }
            }
            ExprKind::Lambda { params, body } => {
                // A parameter in place of a pattern has no name in scope; the names of the
                // pattern come from the binding at the head of the body.
                add(
                    params
                        .iter()
                        .map(|p| p.local)
                        .filter(|&l| hir.locals[l].kind != LocalKind::PatParam)
                        .collect(),
                    span_of(*body),
                );
            }
            ExprKind::Match { arms, .. } | ExprKind::Escape { arms, .. } => {
                for arm in arms {
                    let range = hir.pats[arm.pat].span.range.cover(span_of(arm.body));
                    add(pat_locals(hir, arm.pat), range);
                }
            }
            _ => {}
        }
    }
    let mut visible: Vec<LocalId> = scopes
        .into_iter()
        .filter(|&(local, range)| {
            hir.locals[local].span.module == module && range.contains_inclusive(offset)
        })
        .map(|(local, _)| local)
        .collect();
    // Innermost (bound later) first.
    visible.sort_by_key(|&l| std::cmp::Reverse(hir.locals[l].span.range.start()));
    let mut seen = std::collections::HashSet::new();
    visible.retain(|&l| seen.insert(hir.locals[l].name.clone()));
    visible
}

fn pat_locals(hir: &Program, pat: PatId) -> Vec<LocalId> {
    let mut locals = Vec::new();
    let mut stack = vec![pat];
    while let Some(pat) = stack.pop() {
        match &hir.pats[pat].kind {
            PatKind::Bind(local) => locals.push(*local),
            PatKind::Ctor {
                args: Some(args), ..
            } => stack.extend(args.iter().map(|a| a.pat)),
            PatKind::Tuple(pats) => stack.extend(pats.iter().copied()),
            PatKind::List { elems, rest } => {
                stack.extend(elems.iter().copied());
                if let Some(binding) = rest.and_then(|r| r.binding) {
                    locals.push(binding);
                }
            }
            _ => {}
        }
    }
    locals
}
