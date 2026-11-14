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
use emela_syntax::SyntaxKind;
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
    if let Some(items) = lexical(analysis, module, before) {
        return items;
    }
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
            Some("use" | "uses") => cx.in_scope(offset, |kind| kind == DefKind::Effect),
            Some("with") => cx.in_scope(offset, |kind| {
                matches!(kind, DefKind::Handler | DefKind::Layer)
            }),
            _ => match slot(&before[..start]) {
                Some(Slot::Effect) => cx.in_scope(offset, |kind| kind == DefKind::Effect),
                Some(Slot::Error) => cx.in_scope(offset, |kind| kind == DefKind::Error),
                Some(Slot::Trait) => cx.in_scope(offset, |kind| kind == DefKind::Trait),
                Some(Slot::Type) => {
                    let mut items = cx.in_scope(offset, is_type);
                    items.extend(cx.type_modules());
                    items
                }
                None => {
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
            },
        }
    };
    // For duplicate names, keep the first found (the innermost).
    let mut seen = std::collections::HashSet::new();
    items.retain(|item| seen.insert(item.label.clone()));
    items
}

/// Attributes a user can write (spec `[kw.annotation]`), offered after `@`.
const ATTRIBUTES: &[&str] = &["external", "export", "entry", "test", "deprecated"];

/// Attributes allowed only in the bundled core sources (E0224, E0229), offered there only.
const CORE_ATTRIBUTES: &[&str] = &["intrinsic", "internal"];

/// The language names of `@external` and `@export` (spec `[ext.decl]`, `[ext.export]`; others are
/// E0140).
const LANGUAGES: &[&str] = &["js", "wasm", "c"];

/// Contexts decided by the lexical position alone, before any name lookup: inside a comment or
/// a string nothing is offered; after `@`, attribute names; in the first argument of
/// `@external(` and `@export(`, language names. `None` leaves the position to the name contexts.
///
/// Only the text before the cursor is lexed, so what follows (often not yet written) cannot
/// change the answer. The lexer reports each string and interpolation still open at the end of
/// the input with a range ending there, so the innermost open one (the one opened last) tells
/// whether the cursor is in string text or in the code of an interpolation (`#{ }`). A single-line
/// string closed by a line break is reported with a range ending at that break, not at the end.
fn lexical(analysis: &Analysis, module: ModuleId, before: &str) -> Option<Vec<Completion>> {
    use emela_syntax::{DiagnosticCode, SyntaxKind};
    let lexed = emela_syntax::lex(before);
    let end = TextSize::of(before);
    let in_comment = lexed
        .tokens
        .iter()
        .rfind(|t| t.kind != SyntaxKind::EOF)
        .is_some_and(|t| {
            matches!(t.kind, SyntaxKind::COMMENT | SyntaxKind::DOC_COMMENT) && t.range.end() == end
        });
    let in_string = lexed
        .diagnostics
        .iter()
        .filter(|d| {
            d.range.end() == end
                && matches!(
                    d.code,
                    DiagnosticCode::UnterminatedString | DiagnosticCode::UnterminatedInterpolation
                )
        })
        .max_by_key(|d| d.range.start())
        .is_some_and(|d| d.code == DiagnosticCode::UnterminatedString);
    if in_comment || in_string {
        return Some(Vec::new());
    }
    let head = before.trim_end_matches(is_name_char);
    let keywords = |names: &mut dyn Iterator<Item = &&str>| {
        names
            .map(|name| Completion {
                label: (*name).to_owned(),
                kind: CompletionKind::Keyword,
                detail: None,
                insert_text: None,
                documentation: None,
            })
            .collect()
    };
    if head.ends_with('@') {
        // `intrinsic` and `internal` outside core would only produce E0224 and E0229.
        let core: &[&str] = if analysis.modules.is_core(module) {
            CORE_ATTRIBUTES
        } else {
            &[]
        };
        return Some(keywords(&mut ATTRIBUTES.iter().chain(core)));
    }
    let open = head.trim_end().strip_suffix('(')?;
    if ["@external", "@export"]
        .iter()
        .any(|attr| open.trim_end().ends_with(attr))
    {
        return Some(keywords(&mut LANGUAGES.iter()));
    }
    None
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
            items.extend(self.modules(&imports, None));
        }
        items
    }

    /// Imported modules, and core modules that can be written left of `.` without an import.
    /// With `keep`, only the modules found for which it holds.
    fn modules(
        &self,
        imports: &[(ModuleName, Vec<String>)],
        keep: Option<&dyn Fn(ModuleId) -> bool>,
    ) -> Vec<Completion> {
        let modules = &self.analysis.modules;
        let imported = imports
            .iter()
            .map(|(path, _)| (modules.lookup(path), path.clone()));
        let core = modules
            .iter()
            .filter(|&(id, data)| {
                modules.is_core(id)
                    && data.name.segments().len() == 1
                    && data.name.last() != emela_resolve::PRELUDE
            })
            .map(|(id, data)| (Some(id), data.name.clone()));
        imported
            .chain(core)
            .filter(|&(id, _)| keep.is_none_or(|keep| id.is_some_and(keep)))
            .map(|(_, name)| Completion {
                label: name.last().to_string(),
                kind: CompletionKind::Module,
                documentation: None,
                detail: Some(name.to_string()),
                insert_text: None,
            })
            .collect()
    }

    /// The modules that can be written at the head of a type (`Shapes.Shape`): those with a pub
    /// type, enum or error.
    fn type_modules(&self) -> Vec<Completion> {
        let hir = self.hir();
        let has_types = |module: ModuleId| {
            hir.modules.get(module).is_some_and(|m| {
                m.items.iter().any(|&def| {
                    let data = &hir.defs[def];
                    data.is_pub && is_type(data.kind)
                })
            })
        };
        self.modules(&imports(self.text), Some(&has_types))
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

/// Kinds of definitions whose names can be written as a type (`[grammar.type]`). A trait is
/// written only in bounds, impl heads and derive, so it is not one of them.
fn is_type(kind: DefKind) -> bool {
    matches!(
        kind,
        DefKind::Type | DefKind::BuiltinType | DefKind::Enum | DefKind::Error
    )
}

/// A position in a signature, a declaration head or a type where only names of one kind can be
/// written (`[lsp.completion]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// An item of `uses { … }`, an effect set in type arguments, or the effect after
    /// `handler H:`.
    Effect,
    /// After `fails`, and after `|` in its error set.
    Error,
    /// The trait of an impl head, a supertrait, a bound of a declared type parameter, or a
    /// derive.
    Trait,
    /// The type of a parameter, a field, a binding or a const, a return type, a parameter of a
    /// function type, and a type argument.
    Type,
}

/// The slot at the end of `text` (the text before the partially typed name), decided from its
/// tokens. The source being edited does not parse, so this reads only the shape of the tokens
/// before the cursor. A position that this shape cannot tell apart from an expression (the
/// parameters of a lambda or of `init`, a pattern before `:`) gets `None`, and with it the
/// general candidates.
///
/// One shape is read wrongly: after a lambda whose last parameter has a function type with a
/// `fails` clause (`|f: fn() -> A fails E| `), the body is taken as the error set.
fn slot(text: &str) -> Option<Slot> {
    use SyntaxKind as K;
    let t = Tokens::new(text);
    let end = t.kinds.len();
    let last = end.checked_sub(1).map(|i| t.kinds[i]);

    // `fails E1 | E2 | `: the clause follows a type (`[error.fails]`), which tells the
    // contextual keyword apart from a variable named `fails`.
    let list = t.list_before(end, K::PIPE);
    if t.word_before(list, "fails")
        && list.checked_sub(2).is_some_and(|i| {
            matches!(
                t.kinds[i],
                K::TYPE_NAME | K::UPPER_NAME | K::R_BRACK | K::R_PAREN | K::SELF_TYPE_KW
            )
        })
    {
        return Some(Slot::Error);
    }

    // `impl T:`, `impl T[A]:`, `trait T: A + `, `[A: B + ` in declared type parameters, and
    // `handler H:`, `handler H(…):`.
    let list = t.list_before(end, K::PLUS);
    if let Some(colon) = list.checked_sub(1).filter(|&i| t.kinds[i] == K::COLON) {
        let head = t.group_before(colon).unwrap_or(colon);
        if t.type_ref_before(head)
            .is_some_and(|name| t.kind(name.wrapping_sub(1)) == Some(K::IMPL_KW))
            || (t.kind(colon.wrapping_sub(1)) == Some(K::TYPE_NAME)
                && t.kind(colon.wrapping_sub(2)) == Some(K::TRAIT_KW))
        {
            return Some(Slot::Trait);
        }
        if list == end
            && t.kind(head.wrapping_sub(1)) == Some(K::TYPE_NAME)
            && t.kind(head.wrapping_sub(2)) == Some(K::HANDLER_KW)
        {
            return Some(Slot::Effect);
        }
        if let Some(&open) = t.open.last()
            && t.declares_params(open)
            && matches!(
                t.kind(colon.wrapping_sub(1)),
                Some(K::TYPE_NAME | K::UPPER_NAME)
            )
            && (colon - 2 == open || t.kind(colon - 2) == Some(K::COMMA))
        {
            return Some(Slot::Trait);
        }
    }

    // `derive A, B, ` after the fields or the variants of a declaration (`[trait.derive]`).
    let list = t.list_before(end, K::COMMA);
    if t.word_before(list, "derive")
        && list.checked_sub(2).is_some_and(|i| {
            matches!(
                t.kinds[i],
                K::TYPE_NAME | K::R_PAREN | K::R_BRACE | K::R_BRACK
            )
        })
    {
        return Some(Slot::Trait);
    }

    // At the start of an item inside a bracket still open.
    if let Some(&open) = t.open.last() {
        let item = t.item_start(open, end);
        let enclosing = t.open.len().checked_sub(2).map(|i| t.open[i]);
        match t.kinds[open] {
            K::L_BRACE if item == end => {
                if t.word_before(open, "uses") {
                    return Some(Slot::Effect);
                }
                // `Fiber[A, { `: an effect set as a type argument.
                if enclosing.is_some_and(|e| t.type_args(e))
                    && matches!(t.kind(open - 1), Some(K::L_BRACK | K::COMMA))
                {
                    return Some(Slot::Effect);
                }
            }
            // `List[`, `Map[K, `, and an error set `E1 | ` as a type argument.
            K::L_BRACK if t.type_args(open) && (item == end || last == Some(K::PIPE)) => {
                return Some(Slot::Type);
            }
            // The parameters of a function type, `fn(A, `.
            K::L_PAREN if item == end && t.kind(open.wrapping_sub(1)) == Some(K::FN_KW) => {
                return Some(Slot::Type);
            }
            // A variant with positional fields, `V(A, ` in an enum.
            K::L_PAREN
                if item == end
                    && t.variant(open.wrapping_sub(1), enclosing)
                    && !t.kinds[open + 1..end].contains(&K::COLON) =>
            {
                return Some(Slot::Type);
            }
            _ => {}
        }
    }

    match last {
        // `name: ` in the parameters of a function, the fields of a type, an error, a handler
        // or a variant, a binding at the start of a statement, and `const C: `. A label in a call
        // (`f(x: `) is a value position and is not matched.
        Some(K::COLON) => {
            let name = end.checked_sub(2)?;
            match t.kinds[name] {
                K::UPPER_NAME if t.kind(name.wrapping_sub(1)) == Some(K::CONST_KW) => {
                    Some(Slot::Type)
                }
                K::LOWER_NAME => {
                    let open = t.open.last().copied();
                    let enclosing = t.open.len().checked_sub(2).map(|i| t.open[i]);
                    let first_in =
                        |open: usize| name - 1 == open || t.kind(name - 1) == Some(K::COMMA);
                    let field = open.is_some_and(|open| {
                        t.kinds[open] == K::L_PAREN
                            && first_in(open)
                            && t.declares_fields(open, enclosing)
                    });
                    let binding = (t.line_start[name] || open.is_some_and(|o| name - 1 == o))
                        && open.is_none_or(|open| t.kinds[open] == K::L_BRACE);
                    (field || binding).then_some(Slot::Type)
                }
                _ => None,
            }
        }
        // A return type: `->` after the parameters of a function or a function type. After a
        // pattern (`Some(x) ->` in a match arm) it is a value position.
        Some(K::THIN_ARROW) => {
            let close = end.checked_sub(2)?;
            let open = t.opener[close]?;
            (t.kind(open.wrapping_sub(1)) == Some(K::FN_KW) || t.fn_params(open))
                .then_some(Slot::Type)
        }
        _ => None,
    }
}

/// The tokens of the text before the cursor, without trivia and newlines, and the brackets they
/// leave open.
struct Tokens<'a> {
    kinds: Vec<SyntaxKind>,
    texts: Vec<&'a str>,
    /// Whether the token starts a line (a newline that is not a continuation is before it).
    line_start: Vec<bool>,
    /// For a closing bracket, its opening bracket.
    opener: Vec<Option<usize>>,
    /// The brackets still open at the end, outermost first.
    open: Vec<usize>,
}

impl<'a> Tokens<'a> {
    fn new(text: &'a str) -> Self {
        use SyntaxKind as K;
        let mut tokens = Tokens {
            kinds: Vec::new(),
            texts: Vec::new(),
            line_start: Vec::new(),
            opener: Vec::new(),
            open: Vec::new(),
        };
        let mut line_start = true;
        for token in emela_syntax::lex(text).tokens {
            if token.kind.is_trivia() || token.kind == K::EOF {
                continue;
            }
            if token.kind == K::NEWLINE {
                line_start = true;
                continue;
            }
            let index = tokens.kinds.len();
            let opener = match token.kind {
                K::L_PAREN | K::L_BRACK | K::L_BRACE | K::INTERP_START => {
                    tokens.open.push(index);
                    None
                }
                K::R_PAREN | K::R_BRACK | K::R_BRACE | K::INTERP_END => tokens.open.pop(),
                _ => None,
            };
            tokens.kinds.push(token.kind);
            tokens
                .texts
                .push(&text[std::ops::Range::<usize>::from(token.range)]);
            tokens.line_start.push(std::mem::take(&mut line_start));
            tokens.opener.push(opener);
        }
        tokens
    }

    /// The kind at `i`, if there is a token there (an index below zero wraps and is out of range).
    fn kind(&self, i: usize) -> Option<SyntaxKind> {
        self.kinds.get(i).copied()
    }

    /// Whether the token right before `i` is the contextual keyword `word`.
    fn word_before(&self, i: usize, word: &str) -> bool {
        i.checked_sub(1)
            .is_some_and(|i| self.kinds[i] == SyntaxKind::LOWER_NAME && self.texts[i] == word)
    }

    /// The start of the type reference (`Timeout`, `Http.Timeout`) that ends right before `end`.
    fn type_ref_before(&self, end: usize) -> Option<usize> {
        use SyntaxKind as K;
        let mut start = end.checked_sub(1)?;
        if !matches!(self.kinds[start], K::TYPE_NAME | K::UPPER_NAME) {
            return None;
        }
        while start >= 2 && self.kinds[start - 1] == K::DOT && self.kinds[start - 2] == K::TYPE_NAME
        {
            start -= 2;
        }
        Some(start)
    }

    /// The start of a run of type references each followed by `sep` (`A | B | `) that ends
    /// right before `end`; `end` itself when there is none.
    fn list_before(&self, end: usize, sep: SyntaxKind) -> usize {
        let mut start = end;
        while start > 0 && self.kinds[start - 1] == sep {
            match self.type_ref_before(start - 1) {
                Some(item) => start = item,
                None => break,
            }
        }
        start
    }

    /// The opening bracket of the group that ends right before `end`.
    fn group_before(&self, end: usize) -> Option<usize> {
        self.opener[end.checked_sub(1)?]
    }

    /// The start of the current item inside the bracket `open`: right after its last comma at
    /// that depth, or after `open`.
    fn item_start(&self, open: usize, end: usize) -> usize {
        let mut i = end;
        while i > open + 1 {
            i -= 1;
            if let Some(opener) = self.opener[i] {
                i = opener;
            } else if self.kinds[i] == SyntaxKind::COMMA {
                return i + 1;
            }
        }
        open + 1
    }

    /// Whether the bracket `open` (`[`) holds type arguments: it follows a type reference that
    /// is not the name being declared (`type Box[`, `impl Box[` declare type parameters).
    fn type_args(&self, open: usize) -> bool {
        use SyntaxKind as K;
        self.kinds[open] == K::L_BRACK
            && self.type_ref_before(open).is_some_and(|name| {
                !matches!(
                    self.kind(name.wrapping_sub(1)),
                    Some(K::TYPE_KW | K::ENUM_KW | K::EFFECT_KW | K::IMPL_KW | K::TRAIT_KW)
                )
            })
    }

    /// Whether the bracket `open` (`[`) declares type parameters: `fn f[`, `type T[`,
    /// `enum T[`, `effect T[` or `impl T[`.
    fn declares_params(&self, open: usize) -> bool {
        use SyntaxKind as K;
        self.kinds[open] == K::L_BRACK
            && match self.kind(open.wrapping_sub(1)) {
                Some(K::LOWER_NAME) => self.kind(open.wrapping_sub(2)) == Some(K::FN_KW),
                Some(K::TYPE_NAME) => matches!(
                    self.kind(open.wrapping_sub(2)),
                    Some(K::TYPE_KW | K::ENUM_KW | K::EFFECT_KW | K::IMPL_KW)
                ),
                _ => false,
            }
    }

    /// Whether the bracket `open` (`(`) holds the parameters of a declared function:
    /// `fn f(` or `fn f[A](`.
    fn fn_params(&self, open: usize) -> bool {
        use SyntaxKind as K;
        let name = match self.kind(open.wrapping_sub(1)) {
            Some(K::R_BRACK) => self.opener[open - 1].map_or(usize::MAX, |o| o.wrapping_sub(1)),
            _ => open.wrapping_sub(1),
        };
        self.kind(name) == Some(K::LOWER_NAME) && self.kind(name.wrapping_sub(1)) == Some(K::FN_KW)
    }

    /// Whether the bracket `open` (`(`) holds `name: type` pairs of a declaration: the
    /// parameters of a function, or the fields of a type, an error, a handler or a variant.
    /// `enclosing` is the bracket around `open`.
    fn declares_fields(&self, open: usize, enclosing: Option<usize>) -> bool {
        use SyntaxKind as K;
        if self.fn_params(open) || self.variant(open.wrapping_sub(1), enclosing) {
            return true;
        }
        let name = match self.kind(open.wrapping_sub(1)) {
            Some(K::R_BRACK) => self.opener[open - 1].map_or(usize::MAX, |o| o.wrapping_sub(1)),
            _ => open.wrapping_sub(1),
        };
        self.kind(name) == Some(K::TYPE_NAME)
            && matches!(
                self.kind(name.wrapping_sub(1)),
                Some(K::TYPE_KW | K::ERROR_KW | K::HANDLER_KW)
            )
    }

    /// Whether `name` is the name of a variant in the body of an enum, the bracket `enclosing`.
    fn variant(&self, name: usize, enclosing: Option<usize>) -> bool {
        use SyntaxKind as K;
        let Some(body) = enclosing else {
            return false;
        };
        if self.kind(name) != Some(K::TYPE_NAME)
            || !(name - 1 == body || self.kinds[name - 1] == K::COMMA || self.line_start[name])
            || self.kinds[body] != K::L_BRACE
        {
            return false;
        }
        let head = match self.kind(body.wrapping_sub(1)) {
            Some(K::R_BRACK) => self.opener[body - 1].map_or(usize::MAX, |o| o.wrapping_sub(1)),
            _ => body.wrapping_sub(1),
        };
        self.kind(head) == Some(K::TYPE_NAME) && self.kind(head.wrapping_sub(1)) == Some(K::ENUM_KW)
    }
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
                    // A local fn's name from its declaration to the end of the block, its own
                    // body included; its parameters in its body (`[fn.local]`).
                    if let Stmt::LocalFn(f) = stmt {
                        let start = hir.locals[f.local].span.range.start();
                        if start <= span.range.end() {
                            add(vec![f.local], TextRange::new(start, span.range.end()));
                        }
                        add(
                            f.sig.params.iter().map(|p| p.local).collect(),
                            span_of(f.body),
                        );
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
