//! Doc comments shared by API documentation and IDE queries (spec 2.2).
//!
//! HIR does not retain comments. Name spans join definitions to reparsed source, including
//! constructors sharing their type's name. Fields have no DefId, so they use their own spans.

use std::cell::RefCell;
use std::collections::HashMap;

use emela_resolve::ModuleId;
use emela_resolve::hir::{DefId, Fields, Item, Span};
use emela_syntax::ast::{self, AstNode, HasAttrs};
use line_index::TextRange;

use crate::frontend::TypedProgram;
use crate::pipeline::Analysis;

/// A request-local cache. Each queried module is parsed at most once, not once per candidate.
#[derive(Default)]
pub(crate) struct Documentation {
    modules: RefCell<HashMap<ModuleId, HashMap<TextRange, String>>>,
}

impl Documentation {
    pub(crate) fn definition(
        &self,
        program: &TypedProgram,
        analysis: &Analysis,
        def: DefId,
    ) -> Option<String> {
        self.at(analysis, program.hir.defs[def].span?)
    }

    pub(crate) fn field(
        &self,
        program: &TypedProgram,
        analysis: &Analysis,
        owner: DefId,
        name: &str,
    ) -> Option<String> {
        let field = fields(program.hir.item(owner)?)?
            .iter()
            .find(|f| f.name == name)?;
        self.at(analysis, field.span)
    }

    pub(crate) fn at(&self, analysis: &Analysis, span: Span) -> Option<String> {
        let mut modules = self.modules.borrow_mut();
        let docs = modules.entry(span.module).or_insert_with(|| {
            let Some(&file) = analysis.files.get(span.module) else {
                return HashMap::new();
            };
            module_docs(&emela_syntax::parse(analysis.sources[file].text()).tree())
        });
        docs.get(&span.range)
            .filter(|doc| !doc.trim().is_empty())
            .cloned()
    }
}

pub(crate) fn fields(item: &Item) -> Option<&[emela_resolve::hir::FieldDef]> {
    match item {
        Item::Type(t) => match &t.fields {
            Fields::Named(fields) => Some(fields),
            _ => None,
        },
        Item::Ctor(c) => match &c.fields {
            Fields::Named(fields) => Some(fields),
            _ => None,
        },
        Item::Error(e) => match &e.fields {
            Fields::Named(fields) => Some(fields),
            _ => None,
        },
        Item::Handler(h) => Some(&h.fields),
        _ => None,
    }
}

pub(crate) fn module_docs(tree: &ast::SourceFile) -> HashMap<TextRange, String> {
    let mut docs = HashMap::new();
    for node in tree.syntax().descendants() {
        macro_rules! collect {
            ($($ty:ty),* $(,)?) => {
                $(if let Some(it) = <$ty>::cast(node.clone()) {
                    let doc = doc_text(&it);
                    if !doc.is_empty()
                        && let Some(name) = it.name()
                    {
                        docs.insert(name.text_range(), doc);
                    }
                    continue;
                })*
            };
        }
        collect!(
            ast::FnDecl,
            ast::TypeDecl,
            ast::EnumDecl,
            ast::ErrorDecl,
            ast::ConstDecl,
            ast::EffectDecl,
            ast::HandlerDecl,
            ast::LayerDecl,
            ast::TraitDecl,
            ast::OpSig,
            ast::Variant,
            ast::Field,
        );
    }
    docs
}

/// A written function signature when type checking has not produced one yet (trait/impl
/// functions in particular). Comments, annotations and the body are left out.
pub(crate) fn written_signature(analysis: &Analysis, span: Span) -> Option<String> {
    let &file = analysis.files.get(span.module)?;
    let text = analysis.sources[file].text();
    let tree = emela_syntax::parse(text).tree();
    let decl = tree
        .syntax()
        .descendants()
        .filter_map(ast::FnDecl::cast)
        .find(|f| f.name().is_some_and(|n| n.text_range() == span.range))?;
    let start = decl
        .syntax()
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| {
            matches!(
                t.kind(),
                emela_syntax::SyntaxKind::SUSPEND_KW | emela_syntax::SyntaxKind::FN_KW
            )
        })?
        .text_range()
        .start();
    let end = decl.body().map_or(decl.syntax().text_range().end(), |b| {
        b.syntax().text_range().start()
    });
    Some(text[usize::from(start)..usize::from(end)].trim().to_owned())
}

/// Unnamed declarations have no name token to join: show their docs only on the header,
/// never on an expression in the body.
pub(crate) fn unnamed_doc(
    analysis: &Analysis,
    module: ModuleId,
    offset: line_index::TextSize,
) -> Option<(TextRange, String, String)> {
    use emela_syntax::SyntaxKind::*;
    let &file = analysis.files.get(module)?;
    let text = analysis.sources[file].text();
    let tree = emela_syntax::parse(text).tree();
    for node in tree.syntax().descendants() {
        if !node.text_range().contains(offset) {
            continue;
        }
        let (start, end) = match node.kind() {
            IMPL_DECL => {
                let start = node
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| t.kind() == IMPL_KW)?
                    .text_range()
                    .start();
                let end = node
                    .children()
                    .find(|n| n.kind() == ITEM_LIST)?
                    .text_range()
                    .start();
                (start, end)
            }
            HANDLER_INIT | HANDLER_RELEASE => {
                let keyword = node
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| t.kind() == LOWER_NAME)?;
                (keyword.text_range().start(), keyword.text_range().end())
            }
            _ => continue,
        };
        let range = TextRange::new(start, end);
        if range.contains(offset) {
            let doc = syntax_doc_text(&node);
            if !doc.trim().is_empty() {
                return Some((
                    range,
                    text[usize::from(start)..usize::from(end)].trim().to_owned(),
                    doc,
                ));
            }
        }
    }
    None
}

/// The definition whose name is at `range` in `module`.
pub(crate) fn def_at(program: &TypedProgram, module: ModuleId, range: TextRange) -> Option<DefId> {
    program.hir.defs.iter().find_map(|(def, data)| {
        data.span
            .is_some_and(|s| s.module == module && s.range == range)
            .then_some(def)
    })
}

/// Strip the marker and one space; keep further indentation for Markdown code blocks.
pub(crate) fn doc_text(node: &impl HasAttrs) -> String {
    syntax_doc_text(node.syntax())
}

fn syntax_doc_text(node: &emela_syntax::SyntaxNode) -> String {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == emela_syntax::SyntaxKind::DOC_COMMENT)
        .map(|t| {
            let line = t.text().strip_prefix("##").unwrap_or(t.text());
            line.strip_prefix(' ').unwrap_or(line).to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
