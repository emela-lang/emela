//! API documentation (`emela doc`). Collects the `pub` declarations of each module (spec 4.3)
//! with their `##` doc comments (spec 2.2) and signatures, including fails and use (spec 9.2),
//! so that people adding a dependency can see what each function uses.
//!
//! The HIR does not keep doc comments, so each module is parsed again and its declarations are
//! joined to their definitions by the span of the name (as in inlay hints).

mod html;
mod markdown;

pub use html::render_html;
pub use markdown::render_markdown;

use emela_resolve::ModuleId;
use emela_resolve::hir::{DefId, FnSig, Item, Res, TypeParam};
use emela_syntax::ast::{self, HasAttrs};
use emela_types::Ty;
use smol_str::SmolStr;

use crate::documentation::{def_at, doc_text, module_docs};
use crate::frontend::TypedProgram;
use crate::pipeline::Analysis;

/// The documentation of one module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocModule {
    /// `Std.Clock`.
    pub name: String,
    /// In source order.
    pub items: Vec<DocItem>,
}

/// A `pub` top-level declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocItem {
    pub name: SmolStr,
    pub kind: DocKind,
    /// `fn now() -> Instant use { Clock }`, `type User(id: Int)`, ...
    pub signature: String,
    /// The `##` lines without the marker, joined by newlines. Markdown.
    pub doc: String,
    /// Variants of an enum, fields of a type, error or handler, operations of an effect, or
    /// functions of a trait. Empty for opaque types (spec 4.4).
    pub children: Vec<DocChild>,
}

/// A variant, field, effect operation or trait function. The parent's kind tells which.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocChild {
    pub name: SmolStr,
    pub signature: String,
    pub doc: String,
}

/// The kind of a top-level declaration that can be `pub` (spec 4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    Type,
    Enum,
    Error,
    Effect,
    Handler,
    Layer,
    Trait,
    Const,
    Fn,
}

impl DocKind {
    /// Types come first on a page, then values (as in Gleam's docs).
    pub fn is_type(self) -> bool {
        !matches!(self, DocKind::Const | DocKind::Fn)
    }
}

/// The documentation of the bundled core and std modules if `core`, otherwise of the
/// project's modules. Sorted by module name.
pub fn collect_docs(program: &TypedProgram, analysis: &Analysis, core: bool) -> Vec<DocModule> {
    let mut modules: Vec<DocModule> = analysis
        .files
        .iter()
        .filter(|&(m, _)| analysis.modules.is_core(m) == core && !analysis.modules.is_dependency(m))
        .map(|(m, &file)| collect_module(program, m, analysis.sources[file].text()))
        .collect();
    modules.sort_by(|a, b| a.name.cmp(&b.name));
    modules
}

fn collect_module(program: &TypedProgram, module: ModuleId, text: &str) -> DocModule {
    let mut items = Vec::new();
    let tree = emela_syntax::parse(text).tree();
    let docs = module_docs(&tree);
    for item in tree.items() {
        let (kind, is_pub, name) = match &item {
            ast::Item::Fn(it) => (DocKind::Fn, it.is_pub(), it.name()),
            ast::Item::Type(it) => (DocKind::Type, it.is_pub(), it.name()),
            ast::Item::Enum(it) => (DocKind::Enum, it.is_pub(), it.name()),
            ast::Item::Error(it) => (DocKind::Error, it.is_pub(), it.name()),
            ast::Item::Const(it) => (DocKind::Const, it.is_pub(), it.name()),
            ast::Item::Effect(it) => (DocKind::Effect, it.is_pub(), it.name()),
            ast::Item::Handler(it) => (DocKind::Handler, it.is_pub(), it.name()),
            ast::Item::Layer(it) => (DocKind::Layer, it.is_pub(), it.name()),
            ast::Item::Trait(it) => (DocKind::Trait, it.is_pub(), it.name()),
            ast::Item::Import(_) | ast::Item::Impl(_) => continue,
        };
        if !is_pub {
            continue;
        }
        let Some(name) = name else { continue };
        // Declarations without a definition (after a syntax error) are skipped.
        let Some(def) = def_at(program, module, name.text_range()) else {
            continue;
        };
        let (signature, children) = describe(program, module, &item, def);
        items.push(DocItem {
            name: program.hir.defs[def].name.clone(),
            kind,
            signature,
            doc: docs.get(&name.text_range()).cloned().unwrap_or_default(),
            children,
        });
    }
    DocModule {
        name: program.hir.modules[module].name.to_string(),
        items,
    }
}

/// The signature of a declaration and its children: variants, fields, operations or trait
/// functions. Opaque types show only their name and type parameters (spec 4.4).
fn describe(
    program: &TypedProgram,
    module: ModuleId,
    item: &ast::Item,
    def: DefId,
) -> (String, Vec<DocChild>) {
    let hir = &program.hir;
    let data = &hir.defs[def];
    let name = &data.name;
    let opaque = if data.is_opaque { "opaque " } else { "" };
    match (item, hir.item(def)) {
        (ast::Item::Fn(_), Some(Item::Fn(it))) => (
            fn_signature(program, def, &it.sig).unwrap_or_default(),
            Vec::new(),
        ),
        (ast::Item::Const(_), _) => (const_signature(program, def), Vec::new()),
        (ast::Item::Type(decl), Some(Item::Type(it))) => {
            let head = format!("{opaque}type {name}{}", type_params(hir, &it.type_params));
            match it.ctor {
                Some(ctor) if !data.is_opaque => (
                    head + &ctor_fields(program, ctor),
                    field_children(program, ctor, decl.field_list()),
                ),
                _ => (head, Vec::new()),
            }
        }
        // An error is its own constructor.
        (ast::Item::Error(decl), Some(Item::Error(_))) => (
            format!("error {name}{}", ctor_fields(program, def)),
            field_children(program, def, decl.field_list()),
        ),
        (ast::Item::Enum(decl), Some(Item::Enum(it))) => {
            let head = format!("{opaque}enum {name}{}", type_params(hir, &it.type_params));
            if data.is_opaque {
                return (head, Vec::new());
            }
            let variants = decl
                .variants()
                .filter_map(|v| {
                    let variant = def_at(program, module, v.name()?.text_range())?;
                    let variant_name = hir.defs[variant].name.clone();
                    Some(DocChild {
                        signature: format!("{variant_name}{}", ctor_fields(program, variant)),
                        name: variant_name,
                        doc: doc_text(&v),
                    })
                })
                .collect();
            (head, variants)
        }
        (ast::Item::Effect(decl), Some(Item::Effect(it))) => {
            let head = format!("effect {name}{}", type_params(hir, &it.type_params));
            let ops = decl
                .ops()
                .filter_map(|op| {
                    let op_def = def_at(program, module, op.name()?.text_range())?;
                    let Some(Item::Op(item)) = hir.item(op_def) else {
                        return None;
                    };
                    Some(DocChild {
                        name: hir.defs[op_def].name.clone(),
                        signature: fn_signature(program, op_def, &item.sig)?,
                        doc: doc_text(&op),
                    })
                })
                .collect();
            (head, ops)
        }
        (ast::Item::Handler(decl), Some(Item::Handler(it))) => (
            crate::hover::handler_text(program, def).unwrap_or_default(),
            it.ctor
                .map(|ctor| field_children(program, ctor, decl.field_list()))
                .unwrap_or_default(),
        ),
        (ast::Item::Layer(_), _) => (
            crate::hover::layer_text(program, def).unwrap_or_default(),
            Vec::new(),
        ),
        (ast::Item::Trait(decl), Some(Item::Trait(it))) => {
            let supers: Vec<&str> = it
                .supertraits
                .iter()
                .filter_map(|(res, _)| def_name(hir, res))
                .collect();
            let mut head = format!("trait {name}");
            if !supers.is_empty() {
                head.push_str(&format!(": {}", supers.join(" + ")));
            }
            let fns = decl
                .fns()
                .filter_map(|f| {
                    let f_def = def_at(program, module, f.name()?.text_range())?;
                    let Some(Item::TraitFn(item)) = hir.item(f_def) else {
                        return None;
                    };
                    Some(DocChild {
                        name: hir.defs[f_def].name.clone(),
                        signature: fn_signature(program, f_def, &item.sig)?,
                        doc: doc_text(&f),
                    })
                })
                .collect();
            (head, fns)
        }
        // The HIR does not match the declaration (after a syntax error).
        _ => (String::new(), Vec::new()),
    }
}

/// The signature of a fn, an effect operation or a trait function:
/// `suspend fn name[A: Ord](label: T, ...) -> R fails E use R`.
///
/// Only the type parameters written on the declaration go in `[...]`. The invisible sets of
/// function-typed parameters (spec 9.2) show as `E(f)` and `R(f)`, as in hover.
fn fn_signature(program: &TypedProgram, def: DefId, sig: &FnSig) -> Option<String> {
    let hir = &program.hir;
    let types = &program.types;
    let cons = &types.ty_table.cons;
    let signature = types.sigs.get(def)?;
    let Ty::Fn(f) = &signature.scheme.ty else {
        return None;
    };
    // The effect's type parameters come first for an operation, and the invisible sets last,
    // so pick the written ones by name.
    let params = &signature.scheme.params;

    let mut text = String::new();
    if sig.is_suspend {
        text.push_str("suspend ");
    }
    text.push_str("fn ");
    text.push_str(&hir.defs[def].name);

    let written: Vec<String> = sig
        .type_params
        .iter()
        .filter_map(|tp| {
            let name = &hir.defs[tp.def].name;
            params.iter().find(|p| &p.name == name)
        })
        .map(|p| {
            if p.bounds.is_empty() {
                p.name.to_string()
            } else {
                format!("{}: {}", p.name, p.bounds.join(" + "))
            }
        })
        .collect();
    if !written.is_empty() {
        text.push_str(&format!("[{}]", written.join(", ")));
    }

    let args: Vec<String> = signature
        .labels
        .iter()
        .zip(&f.params)
        .map(|(label, ty)| {
            format!(
                "{}: {}",
                label.as_deref().unwrap_or("_"),
                ty.display_with(cons, params)
            )
        })
        .collect();
    text.push_str(&format!(
        "({}) -> {}",
        args.join(", "),
        f.ret.display_with(cons, params)
    ));

    if !f.fails.is_empty() {
        text.push_str(&format!(" fails {}", f.fails.display_with(cons, params)));
    }
    if !f.uses.is_empty() {
        text.push_str(&format!(" use {}", f.uses.display_effects(cons, params)));
    }
    Some(text)
}

/// `const NAME: T`, or `const NAME` if the type is unknown.
fn const_signature(program: &TypedProgram, def: DefId) -> String {
    let name = &program.hir.defs[def].name;
    match program.types.sigs.get(def) {
        Some(sig) => format!(
            "const {name}: {}",
            sig.scheme.display(&program.types.ty_table.cons)
        ),
        None => format!("const {name}"),
    }
}

/// `[A, B: Ord]` from the written type parameters, or empty.
fn type_params(hir: &emela_resolve::hir::Program, params: &[TypeParam]) -> String {
    if params.is_empty() {
        return String::new();
    }
    let names: Vec<String> = params
        .iter()
        .map(|tp| {
            let name = hir.defs[tp.def].name.to_string();
            let bounds: Vec<&str> = tp
                .bounds
                .iter()
                .filter_map(|(res, _)| def_name(hir, res))
                .collect();
            if bounds.is_empty() {
                name
            } else {
                format!("{name}: {}", bounds.join(" + "))
            }
        })
        .collect();
    format!("[{}]", names.join(", "))
}

/// The fields of a constructor: `(id: Int, name: String)`, `(Float, Float)` for positional
/// fields, or empty for a constructor without fields.
fn ctor_fields(program: &TypedProgram, ctor: DefId) -> String {
    let cons = &program.types.ty_table.cons;
    let Some(sig) = program.types.sigs.get(ctor) else {
        return String::new();
    };
    let Ty::Fn(f) = &sig.scheme.ty else {
        return String::new();
    };
    let params = &sig.scheme.params;
    let fields: Vec<String> = f
        .params
        .iter()
        .enumerate()
        .map(|(i, ty)| match sig.labels.get(i).cloned().flatten() {
            Some(label) => format!("{label}: {}", ty.display_with(cons, params)),
            None => ty.display_with(cons, params).to_string(),
        })
        .collect();
    format!("({})", fields.join(", "))
}

/// One child per named field, pairing the `##` in the AST with the type from the constructor
/// (fields are not definitions, so they are matched by position).
fn field_children(
    program: &TypedProgram,
    ctor: DefId,
    list: Option<ast::FieldList>,
) -> Vec<DocChild> {
    let cons = &program.types.ty_table.cons;
    let (Some(list), Some(sig)) = (list, program.types.sigs.get(ctor)) else {
        return Vec::new();
    };
    let Ty::Fn(f) = &sig.scheme.ty else {
        return Vec::new();
    };
    list.fields()
        .zip(&f.params)
        .filter_map(|(field, ty)| {
            let name = SmolStr::new(field.name()?.text());
            Some(DocChild {
                signature: format!("{name}: {}", ty.display_with(cons, &sig.scheme.params)),
                name,
                doc: doc_text(&field),
            })
        })
        .collect()
}

/// The name of a resolved definition (a trait bound or supertrait).
fn def_name<'a>(hir: &'a emela_resolve::hir::Program, res: &Res) -> Option<&'a str> {
    match res {
        Res::Def(def) => Some(hir.defs[*def].name.as_str()),
        _ => None,
    }
}
