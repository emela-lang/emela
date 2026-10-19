//! Name navigation (LSP go to definition, find references, document symbols).
//!
//! Go to definition and find references look up the resolver's reference table
//! (`Program::refs`) and the type checker's operation table (`ProgramTypes::ops`). Document
//! symbols come from the syntax tree alone.

use emela_resolve::ModuleId;
use emela_resolve::hir::{DefId, DefKind, ExprKind, Item, LocalId, PatKind, RefTarget, Res, Span};
use emela_syntax::ast::{self, AstNode};
use emela_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};
use emela_types::Ty;
use line_index::{TextRange, TextSize};

use crate::frontend::TypedProgram;
use crate::pipeline::Analysis;
use crate::source::FileId;

/// A position in source: a navigation target or a reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NavTarget {
    pub file: FileId,
    pub range: TextRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Symbol {
    Def(DefId),
    Local(LocalId),
    Module(ModuleId),
}

/// Definitions in the bundled core and definitions without source (builtin module functions and
/// types, the builtin Prelude) have no target, so `None`. A module's target is the top of its
/// file.
pub(crate) fn definition(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    offset: TextSize,
) -> Option<NavTarget> {
    let module = module_of(analysis, file)?;
    match symbol_at(program, module, offset)? {
        Symbol::Def(def) => target(analysis, program.hir.defs[def].span?),
        Symbol::Local(local) => target(analysis, program.hir.locals[local].span),
        Symbol::Module(m) => target(
            analysis,
            Span {
                module: m,
                range: TextRange::empty(0.into()),
            },
        ),
    }
}

/// With `declaration`, the definition's name is included. References inside the bundled core are
/// excluded. Sorted by file and position.
pub(crate) fn references(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    offset: TextSize,
    declaration: bool,
) -> Vec<NavTarget> {
    let Some(module) = module_of(analysis, file) else {
        return Vec::new();
    };
    let Some(symbol) = symbol_at(program, module, offset) else {
        return Vec::new();
    };
    let spans = occurrences(program, symbol, declaration);
    let mut targets: Vec<NavTarget> = spans
        .into_iter()
        .filter_map(|span| target(analysis, span))
        .collect();
    targets.sort_by_key(|t| (t.file, t.range.start(), t.range.end()));
    targets.dedup();
    targets
}

/// Same-file references, including declarations. Bindings are immutable (spec 6.3), so all
/// occurrences are text highlights rather than reads and writes.
pub(crate) fn document_highlights(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    offset: TextSize,
) -> Vec<TextRange> {
    references(program, analysis, file, offset, true)
        .into_iter()
        .filter(|t| t.file == file)
        .map(|t| t.range)
        .collect()
}

/// The outer nominal type of the name or smallest expression. No target for structural,
/// primitive, unknown or function types, or definitions without source.
pub(crate) fn type_definition(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    offset: TextSize,
) -> Option<NavTarget> {
    let module = module_of(analysis, file)?;
    let types = &program.types;
    let ty = match symbol_at(program, module, offset) {
        Some(Symbol::Local(local)) => types.locals.get(local),
        Some(Symbol::Def(def))
            if matches!(
                program.hir.defs[def].kind,
                DefKind::Type
                    | DefKind::Enum
                    | DefKind::Trait
                    | DefKind::Error
                    | DefKind::Handler
                    | DefKind::Effect
            ) =>
        {
            return target(analysis, program.hir.defs[def].span?);
        }
        _ => None,
    };
    let ty = ty
        .or_else(|| {
            program
                .hir
                .exprs
                .iter()
                .filter(|(_, e)| e.span.module == module && e.span.range.contains(offset))
                .min_by_key(|(_, e)| e.span.range.len())
                .and_then(|(expr, _)| types.exprs.get(expr))
        })
        .or_else(|| match symbol_at(program, module, offset)? {
            Symbol::Def(def) => Some(&types.sigs.get(def)?.scheme.ty),
            _ => None,
        })?;
    let def = match ty {
        Ty::Named(con, _) => types.ty_table.def(*con),
        Ty::Cap(tag) => tag.def,
        _ => return None,
    };
    target(analysis, program.hir.defs[def].span?)
}

/// Explicit impls of a trait, or the matching methods for a trait method. An omitted default
/// method points at the impl block. Generated/builtin impls have no source target.
pub(crate) fn implementations(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    offset: TextSize,
) -> Vec<NavTarget> {
    let Some(module) = module_of(analysis, file) else {
        return Vec::new();
    };
    let Some(Symbol::Def(def)) = symbol_at(program, module, offset) else {
        return Vec::new();
    };
    let data = &program.hir.defs[def];
    let (trait_def, method) = match data.kind {
        DefKind::Trait => (def, None),
        DefKind::TraitFn => {
            let Some(parent) = data.parent else {
                return Vec::new();
            };
            (parent, Some(def))
        }
        _ => return Vec::new(),
    };
    let mut targets: Vec<_> = program
        .hir
        .items
        .iter()
        .filter_map(|(def, item)| {
            let Item::Impl(item) = item else {
                return None;
            };
            if item.trait_ref != Res::Def(trait_def) {
                return None;
            }
            let span = method
                .and_then(|method| {
                    item.fns
                        .iter()
                        .find(|f| f.trait_fn == Res::Def(method))
                        .map(|f| f.name.span)
                })
                .or(program.hir.defs[def].span)?;
            target(analysis, span)
        })
        .collect();
    targets.sort_by_key(|t| (t.file, t.range.start(), t.range.end()));
    targets.dedup();
    targets
}

/// With `declaration`, the definition's name is included.
fn occurrences(program: &TypedProgram, symbol: Symbol, declaration: bool) -> Vec<Span> {
    let hir = &program.hir;
    let same = |other: Symbol| same_symbol(program, symbol, other);
    let mut spans: Vec<Span> = hir
        .refs
        .iter()
        .filter(|r| same(to_symbol(r.target)))
        .map(|r| r.span)
        .collect();
    // Effect operations (the `info` of `log.info`).
    spans.extend(
        program
            .types
            .ops
            .iter()
            .filter(|&(_, &op)| same(Symbol::Def(op)))
            .filter_map(|(expr, _)| match &hir.exprs[expr].kind {
                ExprKind::Field { name, .. } => Some(name.span),
                _ => None,
            }),
    );
    if declaration {
        match symbol {
            Symbol::Def(_) => spans.extend(
                hir.defs
                    .iter()
                    .filter(|&(def, _)| same(Symbol::Def(def)))
                    .filter_map(|(_, data)| data.span),
            ),
            Symbol::Local(local) => spans.push(hir.locals[local].span),
            Symbol::Module(_) => {}
        }
    }
    spans
}

// ---------------------------------------------------------------------------

/// The range of the name to rename. `None` for names that cannot be renamed (modules, `self`,
/// the bundled core, and definitions without source).
pub(crate) fn prepare_rename(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    offset: TextSize,
) -> Option<TextRange> {
    let module = module_of(analysis, file)?;
    let symbol = symbol_at(program, module, offset)?;
    renamable(program, analysis, symbol)?;
    // The range of the name at the position, whether a reference or a definition.
    occurrences(program, symbol, true)
        .into_iter()
        .find(|span| span.module == module && span.range.contains_inclusive(offset))
        .map(|span| span.range)
}

/// The message is shown in the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameError {
    /// A name that cannot be renamed (modules, `self`, the bundled core, definitions without
    /// source), or no name at the position.
    NotRenamable,
    /// The new name is not in the same lexical class as the old one (spec 2.3: lowercase name,
    /// type name, uppercase name).
    InvalidName { name: String, class: &'static str },
}

impl std::fmt::Display for RenameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenameError::NotRenamable => write!(f, "this name cannot be renamed"),
            RenameError::InvalidName { name, class } => {
                write!(f, "`{name}` is not a valid {class}")
            }
        }
    }
}

/// Replacement text for each definition and reference. Punned named arguments and field patterns
/// (`NotFound(id:)`, `User(name:)`) keep the label and become `id: new_name`.
pub(crate) fn rename(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    offset: TextSize,
    new_name: &str,
) -> Result<Vec<(NavTarget, String)>, RenameError> {
    let module = module_of(analysis, file).ok_or(RenameError::NotRenamable)?;
    let symbol = symbol_at(program, module, offset).ok_or(RenameError::NotRenamable)?;
    let kind = renamable(program, analysis, symbol).ok_or(RenameError::NotRenamable)?;
    if lex_one(new_name) != Some(kind) {
        let class = match kind {
            SyntaxKind::TYPE_NAME => "type name",
            SyntaxKind::UPPER_NAME => "uppercase name",
            _ => "lowercase name",
        };
        return Err(RenameError::InvalidName {
            name: new_name.to_owned(),
            class,
        });
    }
    let puns = puns(program);
    let mut edits: Vec<(NavTarget, String)> = occurrences(program, symbol, true)
        .into_iter()
        .filter_map(|span| {
            let text = if puns.contains(&span) {
                let label = &analysis.sources[*analysis.files.get(span.module)?].text()
                    [std::ops::Range::<usize>::from(span.range)];
                format!("{label}: {new_name}")
            } else {
                new_name.to_owned()
            };
            Some((target(analysis, span)?, text))
        })
        .collect();
    edits.sort_by_key(|(t, _)| (t.file, t.range.start()));
    edits.dedup();
    Ok(edits)
}

/// If renamable, the lexical kind of the old name.
fn renamable(program: &TypedProgram, analysis: &Analysis, symbol: Symbol) -> Option<SyntaxKind> {
    let hir = &program.hir;
    let span = match symbol {
        Symbol::Def(def) => {
            let data = &hir.defs[def];
            if matches!(data.kind, DefKind::Impl | DefKind::BuiltinType) {
                return None;
            }
            data.span?
        }
        Symbol::Local(local) => hir.locals[local].span,
        Symbol::Module(_) => return None,
    };
    let file = *analysis.files.get(span.module)?;
    if analysis.modules.is_core(span.module) {
        return None;
    }
    let text = analysis.sources[file].text();
    let kind = lex_one(text.get(std::ops::Range::<usize>::from(span.range))?)?;
    matches!(
        kind,
        SyntaxKind::LOWER_NAME | SyntaxKind::TYPE_NAME | SyntaxKind::UPPER_NAME
    )
    .then_some(kind)
}

/// The kind of the token, if the text is exactly one token.
fn lex_one(text: &str) -> Option<SyntaxKind> {
    let lexed = emela_syntax::lex(text);
    if !lexed.diagnostics.is_empty() {
        return None;
    }
    let mut tokens = lexed
        .tokens
        .iter()
        .filter(|t| !t.kind.is_trivia() && t.kind != SyntaxKind::EOF);
    let token = tokens.next()?;
    (tokens.next().is_none() && token.range.len() == TextSize::of(text)).then_some(token.kind)
}

/// Name positions of punned named arguments (`NotFound(id:)`) and field patterns
/// (`User(name:)`).
fn puns(program: &TypedProgram) -> std::collections::HashSet<Span> {
    let hir = &program.hir;
    let mut puns = std::collections::HashSet::new();
    for (_, expr) in hir.exprs.iter() {
        let args = match &expr.kind {
            ExprKind::Call { args, .. } => args,
            ExprKind::Update { fields, .. } => fields,
            _ => continue,
        };
        puns.extend(
            args.iter()
                .filter(|a| a.punned)
                .map(|a| hir.exprs[a.value].span),
        );
    }
    for (_, pat) in hir.pats.iter() {
        if let PatKind::Ctor {
            args: Some(args), ..
        } = &pat.kind
        {
            for arg in args {
                if let Some(label) = &arg.label
                    && matches!(hir.pats[arg.pat].kind, PatKind::Bind(_))
                    && hir.pats[arg.pat].span == label.span
                {
                    puns.insert(label.span);
                }
            }
        }
    }
    puns
}

/// Looks at references, effect operation references, definition names, and local binding sites,
/// in that order.
///
/// The end of a range (a cursor right after the name) counts too. Ranges containing the position
/// are preferred.
fn symbol_at(program: &TypedProgram, module: ModuleId, offset: TextSize) -> Option<Symbol> {
    let hir = &program.hir;
    let mut found: Vec<(TextRange, Symbol)> = Vec::new();
    let mut add = |span: Span, symbol: Symbol| {
        if span.module == module && span.range.contains_inclusive(offset) {
            found.push((span.range, symbol));
        }
    };
    for r in &hir.refs {
        add(r.span, to_symbol(r.target));
    }
    for (expr, &op) in program.types.ops.iter() {
        if let ExprKind::Field { name, .. } = &hir.exprs[expr].kind {
            add(name.span, Symbol::Def(op));
        }
    }
    for (def, data) in hir.defs.iter() {
        // An impl's span covers the whole block, not a definition name.
        if data.kind != DefKind::Impl
            && let Some(span) = data.span
        {
            add(span, Symbol::Def(def));
        }
    }
    for (local, data) in hir.locals.iter() {
        add(data.span, Symbol::Local(local));
    }
    found
        .iter()
        .find(|(range, _)| range.contains(offset))
        .or_else(|| found.first())
        .map(|&(_, symbol)| symbol)
}

/// Definitions sharing a name position (a type and its constructor, an error tag and its
/// constructor) count as the same.
fn same_symbol(program: &TypedProgram, a: Symbol, b: Symbol) -> bool {
    match (a, b) {
        (Symbol::Def(a), Symbol::Def(b)) => {
            let defs = &program.hir.defs;
            a == b || (defs[a].span.is_some() && defs[a].span == defs[b].span)
        }
        _ => a == b,
    }
}

fn to_symbol(target: RefTarget) -> Symbol {
    match target {
        RefTarget::Def(def) => Symbol::Def(def),
        RefTarget::Local(local) => Symbol::Local(local),
        RefTarget::Module(module) => Symbol::Module(module),
    }
}

/// `None` for the bundled core and modules that were not read.
fn target(analysis: &Analysis, span: Span) -> Option<NavTarget> {
    if analysis.modules.is_core(span.module) {
        return None;
    }
    let file = *analysis.files.get(span.module)?;
    Some(NavTarget {
        file,
        range: span.range,
    })
}

fn module_of(analysis: &Analysis, file: FileId) -> Option<ModuleId> {
    analysis
        .files
        .iter()
        .find(|(_, f)| **f == file)
        .map(|(m, _)| m)
}

// ---------------------------------------------------------------------------

/// A declaration in the file. `range` covers the whole declaration (including doc comments and
/// annotations); `selection` is the name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub range: TextRange,
    pub selection: TextRange,
    pub children: Vec<DocumentSymbol>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Fn,
    Type,
    Enum,
    Variant,
    Error,
    Const,
    Effect,
    /// An effect operation.
    Op,
    Handler,
    Layer,
    Trait,
    Impl,
    /// A fn inside a trait, impl or handler.
    Method,
    Field,
}

/// Declarations in source order. Declarations that parsed are returned even with syntax errors;
/// those whose name cannot be read are skipped.
pub fn document_symbols(text: &str) -> Vec<DocumentSymbol> {
    let parse = emela_syntax::parse(text);
    parse
        .tree()
        .items()
        .filter_map(|item| item_symbol(&item))
        .collect()
}

/// A flat workspace symbol with its containing declaration or module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub target: NavTarget,
    pub container: String,
}

/// All project modules, excluding dependencies and bundled core/std. Matching is a
/// case-insensitive substring of the declaration's name.
pub fn workspace_symbols(analysis: &Analysis, query: &str) -> Vec<WorkspaceSymbol> {
    fn flatten(
        symbols: Vec<DocumentSymbol>,
        file: FileId,
        container: &str,
        query: &str,
        out: &mut Vec<WorkspaceSymbol>,
    ) {
        for symbol in symbols {
            if symbol.name.to_lowercase().contains(query) {
                out.push(WorkspaceSymbol {
                    name: symbol.name.clone(),
                    kind: symbol.kind,
                    target: NavTarget {
                        file,
                        range: symbol.selection,
                    },
                    container: container.to_owned(),
                });
            }
            flatten(symbol.children, file, &symbol.name, query, out);
        }
    }
    let query = query.to_lowercase();
    let mut out = Vec::new();
    for (module, &file) in analysis.files.iter() {
        if analysis.modules.is_core(module) || analysis.modules.is_dependency(module) {
            continue;
        }
        flatten(
            document_symbols(analysis.sources[file].text()),
            file,
            &analysis.module(module).name.to_string(),
            &query,
            &mut out,
        );
    }
    out
}

fn item_symbol(item: &ast::Item) -> Option<DocumentSymbol> {
    use ast::Item;
    Some(match item {
        Item::Import(_) => return None,
        Item::Fn(f) => symbol(f.syntax(), f.name()?, SymbolKind::Fn, Vec::new()),
        Item::Type(t) => symbol(
            t.syntax(),
            t.name()?,
            SymbolKind::Type,
            fields(t.field_list()),
        ),
        Item::Error(e) => symbol(
            e.syntax(),
            e.name()?,
            SymbolKind::Error,
            fields(e.field_list()),
        ),
        Item::Enum(e) => {
            let variants = e
                .variants()
                .filter_map(|v| {
                    Some(symbol(
                        v.syntax(),
                        v.name()?,
                        SymbolKind::Variant,
                        fields(v.field_list()),
                    ))
                })
                .collect();
            symbol(e.syntax(), e.name()?, SymbolKind::Enum, variants)
        }
        Item::Const(c) => symbol(c.syntax(), c.name()?, SymbolKind::Const, Vec::new()),
        Item::Effect(e) => {
            let ops = e
                .ops()
                .filter_map(|op| Some(symbol(op.syntax(), op.name()?, SymbolKind::Op, Vec::new())))
                .collect();
            symbol(e.syntax(), e.name()?, SymbolKind::Effect, ops)
        }
        Item::Handler(h) => {
            let mut children = fields(h.field_list());
            children.extend(methods(h.fns()));
            symbol(h.syntax(), h.name()?, SymbolKind::Handler, children)
        }
        Item::Layer(l) => symbol(l.syntax(), l.name()?, SymbolKind::Layer, Vec::new()),
        Item::Trait(t) => symbol(t.syntax(), t.name()?, SymbolKind::Trait, methods(t.fns())),
        Item::Impl(i) => {
            // There is no name token, so the heading `impl User: Show` is the name and the
            // trait name is the selection.
            let trait_ref = i.trait_ref()?;
            let name = match i.self_ty() {
                Some(ty) => format!("impl {}: {}", ty.text(), trait_ref.syntax().text()),
                None => format!("impl {}", trait_ref.syntax().text()),
            };
            DocumentSymbol {
                name,
                kind: SymbolKind::Impl,
                range: i.syntax().text_range(),
                selection: trait_ref.syntax().text_range(),
                children: methods(i.fns()),
            }
        }
    })
}

fn symbol(
    node: &SyntaxNode,
    name: SyntaxToken,
    kind: SymbolKind,
    children: Vec<DocumentSymbol>,
) -> DocumentSymbol {
    DocumentSymbol {
        name: name.text().to_owned(),
        kind,
        range: node.text_range(),
        selection: name.text_range(),
        children,
    }
}

fn fields(list: Option<ast::FieldList>) -> Vec<DocumentSymbol> {
    let Some(list) = list else {
        return Vec::new();
    };
    list.fields()
        .filter_map(|f| Some(symbol(f.syntax(), f.name()?, SymbolKind::Field, Vec::new())))
        .collect()
}

fn methods(fns: impl Iterator<Item = ast::FnDecl>) -> Vec<DocumentSymbol> {
    fns.filter_map(|f| {
        Some(symbol(
            f.syntax(),
            f.name()?,
            SymbolKind::Method,
            Vec::new(),
        ))
    })
    .collect()
}
