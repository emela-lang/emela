//! Lowers declaration contents to HIR. Holds the local scopes (spec 6.3, 6.5, 7.3, 8.4) and the
//! type parameter scopes.
//!
//! The declarations are lowered here; their types in [`types`], their bodies in [`exprs`].

mod exprs;
mod strings;
mod types;

use emela_syntax::SyntaxKind::*;
use emela_syntax::ast::{self, AstNode, HasAttrs};
use emela_syntax::{SyntaxNode, SyntaxToken};
use rowan::TextRange;
use rustc_hash::FxHashMap;
use smol_str::SmolStr;

use crate::diagnostic::{DiagnosticKind, Expected};
use crate::hir::*;
use crate::resolve::{Ns, Pending, Resolver, child_token, token_name};
use crate::source::ModuleId;
use crate::suggest::best_match;

use strings::string_text;
pub use strings::{string_attr, string_attr_value};

pub(crate) fn lower_item(resolver: &mut Resolver<'_>, pending: Pending) {
    let mut lower = Lower {
        r: resolver,
        module: pending.module,
        locals: Vec::new(),
        type_params: Vec::new(),
        self_type: false,
        handler: None,
        type_arg: false,
        ignored_type_params: Vec::new(),
        member_label: None,
        effect_aliases: FxHashMap::default(),
        alias_receiver: false,
        uses: Vec::new(),
        use_binds: FxHashMap::default(),
        region: 0,
        regions: 0,
        init_of: None,
        inner_names: Vec::new(),
        call_update: None,
    };
    let item = match &pending.item {
        ast::Item::Import(_) => return,
        ast::Item::Fn(f) => {
            let bodyless = f.body().is_none();
            lower
                .r
                .check_intrinsic(pending.module, f.syntax(), bodyless);
            Item::Fn(lower.fn_item(pending.def, f))
        }
        ast::Item::Type(t) => lower.type_item(pending.def, pending.ctor, t),
        ast::Item::Enum(e) => lower.enum_item(pending.def, &pending.children, e),
        ast::Item::Error(e) => {
            let fields = lower.named_fields(e.field_list());
            let derives = lower.derives(e.derive());
            Item::Error(ErrorItem {
                fields: match fields {
                    Some(fields) => Fields::Named(fields),
                    None => Fields::None,
                },
                derives,
            })
        }
        ast::Item::Const(c) => {
            let ty = c.ty().map(|t| lower.lower_type(&t));
            let value = lower.opt_expr(c.value(), c.syntax().text_range());
            Item::Const(ConstItem { ty, value })
        }
        ast::Item::Effect(e) => lower.effect_item(pending.def, &pending.children, e),
        ast::Item::Handler(h) => lower.handler_item(pending.def, pending.ctor, h),
        ast::Item::Layer(l) => {
            let members = l
                .members()
                .filter_map(|s| {
                    let p = s.path()?;
                    let res = lower.resolve_type_ns(&p, Expected::HandlerOrLayer);
                    let args = lower.supply_args(&s, res);
                    let ty_args = lower.type_arg_list(s.type_args());
                    Some(LayerMember {
                        res,
                        span: lower.span(p.syntax().text_range()),
                        args,
                        ty_args,
                    })
                })
                .collect();
            Item::Layer(LayerItem { members })
        }
        ast::Item::Trait(t) => lower.trait_item(&pending.children, t),
        ast::Item::Impl(i) => lower.impl_item(pending.def, i),
    };
    lower.r.insert_item(pending.def, item);
    lower.report_deprecated_uses();
}

/// The name of an effect alias made for `use E` called in place: the effect's name in
/// lower snake case (`log_store` for `LogStore`, `state` for `State[Int]`).
pub(crate) fn alias_name(effect: &str) -> SmolStr {
    let name = effect.split('[').next().unwrap_or(effect);
    let name = name.rsplit('.').next().unwrap_or(name).trim();
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out.into()
}

/// A `use E` written as an expression (W4401, `[effect.use]`), reported when the item is done.
struct DeprecatedUse {
    expr: ExprId,
    range: TextRange,
    /// The effect as written after `use` (`State[Int]`).
    effect: SmolStr,
    form: UseForm,
}

enum UseForm {
    /// `(use E).op(...)` or `(use E).op`.
    InPlace { op: SmolStr },
    /// `x = use E`.
    Binding(LocalId),
    /// Anywhere else: an argument, an element, a return value.
    Value,
}

/// A variable bound to `use E` (`x = use E`), and whether `x <- E` means the same: x is used only
/// as the receiver of operations, in the same function and not inside a `with` that starts after
/// the binding, where an effect alias could reach another handler.
struct UseBind {
    region: u32,
    receiver_only: bool,
}

struct Lower<'r, 'a> {
    r: &'r mut Resolver<'a>,
    module: ModuleId,
    /// Innermost last. Rebinding overwrites the name in the same scope.
    locals: Vec<FxHashMap<SmolStr, LocalId>>,
    type_params: Vec<FxHashMap<SmolStr, DefId>>,
    /// Whether `Self` may be written (inside traits and impls).
    self_type: bool,
    /// The handler being lowered and its constructor, if it has one. `Self` as an expression
    /// names that constructor anywhere inside the handler (`[effect.handler]`); that it is only
    /// used inside init is checked by type inference with the handler's name (E4005).
    handler: Option<(DefId, Option<DefId>)>,
    /// Set while looking up a type in a type argument position of a named type. Effect names may
    /// appear there too (set type arguments, `Handler[NotFound, Db]` in spec 9.2); whether an
    /// effect is allowed in that position is checked by type inference.
    type_arg: bool,
    /// The type parameters written on the local fns being lowered, which are E1213
    /// (`[fn.local]`). Their uses are not reported again as undefined.
    ignored_type_params: Vec<SmolStr>,
    /// The label through which the last handler member was named (`Audit.Open`,
    /// `[effect.label]`); taken by the expression or the item of `with` that names it.
    member_label: Option<DefId>,
    /// The effect aliases declared in the item (`x <- E`), with the effect and its type
    /// arguments (`[effect.use]`).
    effect_aliases: FxHashMap<LocalId, (Res, Vec<TypeId>)>,
    /// Set while the base of `x.op` is lowered: a reference to an effect alias is allowed there.
    alias_receiver: bool,
    /// The `use E` expressions of the item, reported at its end (W4401).
    uses: Vec<DeprecatedUse>,
    /// The variables bound to `use E` in the item ([`UseBind`]).
    use_binds: FxHashMap<LocalId, UseBind>,
    /// The current region: a function body, a lambda, a local fn or a `with` body. A variable
    /// bound to `use E` and used in another region may not be rewritten to an effect alias.
    region: u32,
    /// The number of regions made so far.
    regions: u32,
    /// The handler whose init is being lowered, anonymous functions and local fns inside it
    /// included (W4001, E4014, `[effect.handler]`).
    init_of: Option<InitOf>,
    /// The names of the inner handlers of the handlers being lowered. They are in no scope, so
    /// a constructor pattern with one of them is reported as E4015 instead of an undefined name.
    inner_names: Vec<SmolStr>,
    /// The callee of the record update being lowered and the update as written, taken by the
    /// callee when it names the handler of [`Self::init_of`] (W4001, E4014).
    call_update: Option<(TextRange, CallUpdate)>,
}

/// The handler whose init is being lowered ([`Lower::init_of`]).
struct InitOf {
    handler: DefId,
    ctor: DefId,
    /// An inner handler, whose name is never in scope: written for its value, it is E4014
    /// without a deprecation.
    inner: bool,
    /// The names of its fields, in declaration order.
    fields: Vec<SmolStr>,
}

/// A record update `H(n: 2, ..a)` as written ([`Lower::call_update`]).
struct CallUpdate {
    /// The arguments before `..`: the label, if any, and the text.
    written: Vec<(Option<SmolStr>, String)>,
    /// The text of the expression after `..`, in parentheses unless it is a name.
    base: String,
}

/// Names bound by one pattern (or parameter list). Binding the same name twice is reported.
#[derive(Default)]
struct Binder {
    names: Vec<(SmolStr, LocalId)>,
}

impl Lower<'_, '_> {
    fn span(&self, range: TextRange) -> Span {
        Span {
            module: self.module,
            range,
        }
    }

    fn error(&mut self, range: TextRange, kind: DiagnosticKind) {
        self.r.error(self.module, range, kind);
    }

    /// Records into [`Program::refs`].
    fn refer(&mut self, range: TextRange, target: RefTarget) {
        self.r.refer(self.module, range, target);
    }

    fn alloc_expr(&mut self, kind: ExprKind, range: TextRange) -> ExprId {
        let span = self.span(range);
        self.r.alloc_expr(Expr { kind, span })
    }

    fn alloc_pat(&mut self, kind: PatKind, range: TextRange) -> PatId {
        let span = self.span(range);
        self.r.alloc_pat(Pat { kind, span })
    }

    fn alloc_type(&mut self, kind: TypeKind, range: TextRange) -> TypeId {
        let span = Some(self.span(range));
        self.r.alloc_type(TypeRef { kind, span })
    }

    fn alloc_local(&mut self, name: SmolStr, kind: LocalKind, range: TextRange) -> LocalId {
        let span = self.span(range);
        self.r.alloc_local(LocalData { name, kind, span })
    }

    fn push_scope(&mut self) {
        self.locals.push(FxHashMap::default());
    }

    fn pop_scope(&mut self) {
        self.locals.pop();
    }

    /// The newer of two same-named bindings is the visible one (spec 6.3).
    fn bind_all(&mut self, binder: Binder) {
        if self.locals.is_empty() {
            self.push_scope();
        }
        let scope = self.locals.last_mut().expect("a scope is pushed");
        for (name, local) in binder.names {
            scope.insert(name, local);
        }
    }

    fn bind(
        &mut self,
        binder: &mut Binder,
        name: SmolStr,
        kind: LocalKind,
        range: TextRange,
    ) -> LocalId {
        if let Some(&(_, first)) = binder.names.iter().find(|(n, _)| *n == name) {
            let first = self.r.local(first).span.range;
            self.error(
                range,
                DiagnosticKind::DuplicateBinding {
                    name: name.clone(),
                    first: Some(first),
                },
            );
        }
        let local = self.alloc_local(name.clone(), kind, range);
        binder.names.push((name, local));
        local
    }

    fn lookup_local(&self, name: &str) -> Option<LocalId> {
        self.locals
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn lookup_type_param(&self, name: &str) -> Option<DefId> {
        self.type_params
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    /// `[A: Ord, Item]`. Pushes a type parameter scope; the caller pops it with
    /// `pop_type_params`. Parameters are not told apart by kind (a type, an error set or an
    /// effect set): type inference infers the kind from their uses (`[type.fn]`).
    fn type_params(&mut self, owner: DefId, list: Option<SyntaxNode>) -> Vec<TypeParam> {
        self.type_params_in(owner, list, false).0
    }

    /// Reads the type parameters like [`Self::type_params`]. In an impl head (`impl_head`), a
    /// repeated name or the name of a type is not a type parameter but a malformed head (E3303,
    /// spec 10.2), so neither E2103 nor W2101 is reported; the second value tells whether the head
    /// is malformed. A repeated name still gets a type parameter, so the head keeps its arity.
    fn type_params_in(
        &mut self,
        owner: DefId,
        list: Option<SyntaxNode>,
        impl_head: bool,
    ) -> (Vec<TypeParam>, bool) {
        self.type_params.push(FxHashMap::default());
        let Some(list) = list else {
            return (Vec::new(), false);
        };
        let mut params = Vec::new();
        let mut malformed = false;
        let mut seen: FxHashMap<SmolStr, TextRange> = FxHashMap::default();
        for param in list.children().filter(|n| n.kind() == TYPE_PARAM) {
            let Some(token) = param
                .children_with_tokens()
                .filter_map(|e| e.into_token())
                .find(|t| matches!(t.kind(), UPPER_NAME | TYPE_NAME))
            else {
                continue;
            };
            let (name, range) = token_name(&token);
            if let Some(&first) = seen.get(&name) {
                if !impl_head {
                    self.error(
                        range,
                        DiagnosticKind::Duplicate {
                            name,
                            first: Some(first),
                        },
                    );
                    continue;
                }
                malformed = true;
            }
            seen.insert(name.clone(), range);
            // A type-name type parameter hides an outer type, effect or error of the same name
            // (spec 2.3).
            if token.kind() == TYPE_NAME
                && (self.r.lookup(self.module, Ns::Type, &name).is_some()
                    || self.lookup_type_param(&name).is_some())
            {
                if impl_head {
                    malformed = true;
                } else {
                    self.error(
                        range,
                        DiagnosticKind::ShadowedByTypeParam { name: name.clone() },
                    );
                }
            }
            let def = self.r.alloc_def(
                self.module,
                name.clone(),
                range,
                DefKind::TypeParam,
                Some(owner),
            );
            self.type_params
                .last_mut()
                .expect("a type parameter scope is pushed")
                .insert(name, def);
            params.push((def, param));
        }
        // Constraints are looked up after every type parameter is in scope.
        let params = params
            .into_iter()
            .map(|(def, param)| {
                let bounds = param
                    .children()
                    .filter_map(ast::Path::cast)
                    .map(|p| {
                        let res = self.resolve_type_ns(&p, Expected::Trait);
                        (res, self.span(p.syntax().text_range()))
                    })
                    .collect();
                TypeParam { def, bounds }
            })
            .collect();
        (params, malformed)
    }

    fn pop_type_params(&mut self) {
        self.type_params.pop();
    }

    fn fn_item(&mut self, def: DefId, f: &ast::FnDecl) -> FnItem {
        let type_params = self.type_params(def, f.type_params());
        self.push_scope();
        let params = self.params(f.param_list(), LocalKind::Param);
        let sig = self.signature(f.syntax(), type_params, params, f.is_suspend());
        let body = f.body().map(|b| self.block(&b));
        self.pop_scope();
        self.pop_type_params();
        let entries = f
            .annotations()
            .filter(|a| a.name().is_some_and(|n| n.text() == "entry"))
            .map(|a| EntryAttr {
                adapter: self.entry_adapter(&a),
                span: self.span(a.syntax().text_range()),
            })
            .collect();
        FnItem {
            sig,
            body,
            is_external: f.has_annotation("external"),
            externals: attrs(f, "external", self.module),
            is_intrinsic: f.has_annotation("intrinsic"),
            exports: attrs(f, "export", self.module),
            entries,
        }
    }

    /// What the argument of `@entry(T)` names: a fn of this module (`serve`) or of another one
    /// (`Http.serve`), looked up as a value. `None` if the argument is not one name or path.
    fn entry_adapter(&mut self, annotation: &ast::Annotation) -> Option<Res> {
        let args: Vec<SyntaxNode> = annotation
            .args()
            .filter(|a| a.kind() == ANNOT_ARG)
            .collect();
        let [arg] = args.as_slice() else {
            return None;
        };
        if let Some(path) = arg.children().find(|c| c.kind() == ANNOT_PATH) {
            let tokens: Vec<SyntaxToken> = path
                .children_with_tokens()
                .filter_map(|e| e.into_token())
                .filter(|t| matches!(t.kind(), TYPE_NAME | LOWER_NAME))
                .collect();
            let (last, prefix) = tokens.split_last()?;
            if last.kind() != LOWER_NAME {
                // `@entry(Http.)`: the parser reported it.
                return Some(Res::Err);
            }
            let (name, range) = token_name(last);
            let res = match self.resolve_prefix(prefix) {
                Some(left) => match self.member_of_left(&left, Ns::Value, &name, range) {
                    Some(def) => {
                        self.refer(range, RefTarget::Def(def));
                        Res::Def(def)
                    }
                    None => Res::Err,
                },
                None => Res::Err,
            };
            return Some(res);
        }
        let mut tokens = arg
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| !t.kind().is_trivia());
        match (tokens.next(), tokens.next(), arg.first_child()) {
            (Some(name), None, None) if name.kind() == LOWER_NAME => {
                let (name, range) = token_name(&name);
                Some(self.value(&name, range, LOWER_NAME))
            }
            _ => None,
        }
    }

    /// `-> T fails E uses { R }`. `node` is a FN_DECL or OP_SIG.
    fn signature(
        &mut self,
        node: &SyntaxNode,
        type_params: Vec<TypeParam>,
        params: Vec<Param>,
        is_suspend: bool,
    ) -> FnSig {
        let ret = node
            .children()
            .find(|n| n.kind() == RET_TYPE)
            .and_then(|n| n.first_child())
            .map(|t| self.lower_type(&t));
        let fails = node
            .children()
            .find(|n| n.kind() == FAILS_CLAUSE)
            .map(|c| self.error_set(&c));
        let uses = node
            .children()
            .find(|n| n.kind() == USES_CLAUSE)
            .map(|c| self.effect_set(&c));
        FnSig {
            type_params,
            params,
            ret,
            fails,
            uses,
            is_suspend,
        }
    }

    /// Adds them to the current scope.
    fn params(&mut self, list: Option<ast::ParamList>, kind: LocalKind) -> Vec<Param> {
        let Some(list) = list else {
            return Vec::new();
        };
        let mut binder = Binder::default();
        let mut params = Vec::new();
        for param in list.params() {
            let (name, range, local_kind) = if let Some(token) = param.name() {
                let (name, range) = token_name(&token);
                (name, range, kind)
            } else if let Some(token) = child_token(param.syntax(), SELF_KW) {
                // `self` of impls and traits. The parser does not report a misplaced `self`, so
                // it is checked here.
                if !self.self_type {
                    self.error(
                        token.text_range(),
                        DiagnosticKind::SelfOutside { self_type: false },
                    );
                }
                ("self".into(), token.text_range(), LocalKind::SelfParam)
            } else {
                continue;
            };
            let ty = param.ty().map(|t| self.lower_type(&t));
            let local = self.bind(&mut binder, name, local_kind, range);
            let borrowed = param
                .syntax()
                .children()
                .filter(|n| n.kind() == ANNOTATION)
                .find(|n| {
                    n.children_with_tokens()
                        .filter_map(|t| t.into_token())
                        .any(|t| t.kind() == LOWER_NAME && t.text() == "borrowed")
                })
                .map(|n| self.span(n.text_range()));
            params.push(Param {
                local,
                ty,
                borrowed,
            });
        }
        self.bind_all(binder);
        params
    }

    /// Parameters of a lambda. A pattern in place of a name becomes a hidden parameter
    /// ([`LocalKind::PatParam`]) and a binding `pattern = parameter` to put at the head of the
    /// body (spec 6.1), so type inference checks it as a binding (E3108). Names bound by all
    /// parameters share one binder (E2109).
    fn lambda_params(&mut self, list: Option<ast::ParamList>) -> (Vec<Param>, Vec<Stmt>) {
        let Some(list) = list else {
            return (Vec::new(), Vec::new());
        };
        let mut binder = Binder::default();
        let mut params = Vec::new();
        let mut lets = Vec::new();
        for param in list.params() {
            let ty = param.ty().map(|t| self.lower_type(&t));
            if let Some(token) = param.name() {
                let (name, range) = token_name(&token);
                let local = self.bind(&mut binder, name, LocalKind::LambdaParam, range);
                params.push(Param {
                    local,
                    ty,
                    borrowed: None,
                });
            } else if let Some(token) = child_token(param.syntax(), SELF_KW) {
                // As in [`Self::params`].
                if !self.self_type {
                    self.error(
                        token.text_range(),
                        DiagnosticKind::SelfOutside { self_type: false },
                    );
                }
                let local = self.bind(
                    &mut binder,
                    "self".into(),
                    LocalKind::SelfParam,
                    token.text_range(),
                );
                params.push(Param {
                    local,
                    ty,
                    borrowed: None,
                });
            } else if let Some(node) = param.pattern() {
                let range = node.text_range();
                let pat = self.pat(&node, &mut binder);
                let local =
                    self.alloc_local(node.text().to_string().into(), LocalKind::PatParam, range);
                let value = self.alloc_expr(ExprKind::Path(Res::Local(local)), range);
                params.push(Param {
                    local,
                    ty,
                    borrowed: None,
                });
                lets.push(Stmt::Let {
                    pat,
                    ty: None,
                    value,
                });
            }
        }
        self.bind_all(binder);
        (params, lets)
    }

    fn named_fields(&mut self, list: Option<ast::FieldList>) -> Option<Vec<FieldDef>> {
        let list = list?;
        let mut seen: FxHashMap<SmolStr, TextRange> = FxHashMap::default();
        let mut fields = Vec::new();
        for field in list.fields() {
            let Some(token) = field.name() else { continue };
            let (name, range) = token_name(&token);
            if let Some(&first) = seen.get(&name) {
                self.error(
                    range,
                    DiagnosticKind::Duplicate {
                        name: name.clone(),
                        first: Some(first),
                    },
                );
            }
            seen.insert(name.clone(), range);
            let ty = match field.ty() {
                Some(t) => self.lower_type(&t),
                None => self.alloc_type(TypeKind::Missing, range),
            };
            fields.push(FieldDef {
                name,
                ty,
                span: self.span(range),
                key: string_attr(&field, "key"),
            });
        }
        Some(fields)
    }

    fn derives(&mut self, clause: Option<ast::DeriveClause>) -> Vec<(Res, Span)> {
        let Some(clause) = clause else {
            return Vec::new();
        };
        clause
            .traits()
            .map(|p| {
                let res = self.resolve_type_ns(&p, Expected::Trait);
                (res, self.span(p.syntax().text_range()))
            })
            .collect()
    }

    fn type_item(&mut self, def: DefId, ctor: Option<DefId>, t: &ast::TypeDecl) -> Item {
        let type_params = self.type_params(
            def,
            t.syntax().children().find(|n| n.kind() == TYPE_PARAM_LIST),
        );
        let fields = match self.named_fields(t.field_list()) {
            Some(fields) => Fields::Named(fields),
            None => Fields::None,
        };
        let bound = self.bound(t.syntax());
        let derives = self.derives(t.derive());
        self.pop_type_params();
        if let Some(ctor) = ctor {
            self.r.insert_item(
                ctor,
                Item::Ctor(CtorItem {
                    owner: def,
                    fields: fields.clone(),
                }),
            );
        }
        Item::Type(TypeItem {
            type_params,
            ctor,
            fields,
            derives,
            bound,
            // Outside the bundled sources, `@intrinsic` is E2201 and the type stays a user type.
            is_intrinsic: t.has_annotation("intrinsic") && self.r.is_core(self.module),
        })
    }

    fn enum_item(&mut self, def: DefId, variants: &[DefId], e: &ast::EnumDecl) -> Item {
        let type_params = self.type_params(
            def,
            e.syntax().children().find(|n| n.kind() == TYPE_PARAM_LIST),
        );
        let bound = self.bound(e.syntax());
        // Variants with unreadable names were skipped during collection, so match up only the
        // named ones, in order.
        let syntax_variants: Vec<_> = e.variants().filter(|v| v.name().is_some()).collect();
        let tags = syntax_variants
            .iter()
            .map(|v| string_attr(v, "tag"))
            .collect();
        for (&variant, syntax) in variants.iter().zip(&syntax_variants) {
            let fields = if let Some(fields) = self.named_fields(syntax.field_list()) {
                Fields::Named(fields)
            } else {
                let tys: Vec<_> = syntax.tuple_fields().collect();
                if tys.is_empty() {
                    Fields::None
                } else {
                    Fields::Positional(tys.iter().map(|t| self.lower_type(t)).collect())
                }
            };
            self.r
                .insert_item(variant, Item::Ctor(CtorItem { owner: def, fields }));
        }
        let derives = self.derives(e.derive());
        self.pop_type_params();
        Item::Enum(EnumItem {
            type_params,
            variants: variants.to_vec(),
            derives,
            bound,
            tags,
        })
    }

    /// The effect of `uses E` on a type or enum declaration (`[type.rules]`). The clause names
    /// one effect; further ones are ignored.
    fn bound(&mut self, decl: &SyntaxNode) -> Option<EffectRef> {
        let clause = decl.children().find(|n| n.kind() == USES_CLAUSE)?;
        self.effect_set(&clause).items.into_iter().next()
    }

    fn effect_item(&mut self, def: DefId, children: &[DefId], e: &ast::EffectDecl) -> Item {
        // An effect's type parameters (spec 8.1) are usable in its operation signatures.
        let type_params = self.type_params(def, e.type_params());
        let (members, ops): (Vec<DefId>, Vec<DefId>) = children
            .iter()
            .partition(|&&d| self.r.def(d).kind == DefKind::Member);
        let ops = &ops[..];
        // Handler members (`[effect.decl]`): the parameters of the init, its `fails`, and the
        // effect the handler implements. Duplicates were skipped during collection.
        for m in e.members() {
            let Some(name) = m.name() else { continue };
            let Some(&def) = members.iter().find(|&&d| {
                let data = &self.r.def(d);
                data.name == name.text() && data.span.map(|s| s.range) == Some(name.text_range())
            }) else {
                continue;
            };
            self.push_scope();
            let params = self.params(m.param_list(), LocalKind::Param);
            let sig = self.signature(m.syntax(), Vec::new(), params, m.is_suspend());
            self.pop_scope();
            let effects = self.handler_effects(m.effects());
            self.r
                .insert_item(def, Item::Member(MemberItem { sig, effects }));
        }
        // Duplicate operations were skipped during collection, so match by name.
        for op in e.ops() {
            let Some(name) = op.name() else { continue };
            let Some(&def) = ops.iter().find(|&&d| {
                let data = &self.r.def(d);
                data.name == name.text() && data.span.map(|s| s.range) == Some(name.text_range())
            }) else {
                continue;
            };
            // An operation's own type parameters (`fork[A]`, spec 8.1, 11.2). Only built-in
            // effects may have them; type inference reports E1213 otherwise.
            let op_params = self.type_params(def, op.type_params());
            self.push_scope();
            let params = self.params(op.param_list(), LocalKind::Param);
            let sig = self.signature(op.syntax(), op_params, params, op.is_suspend());
            self.pop_type_params();
            self.pop_scope();
            self.r.insert_item(def, Item::Op(OpItem { sig }));
        }
        self.pop_type_params();
        let label = e.label().map(|l| LabelOf {
            base: match l.base() {
                Some(p) => self.resolve_type_ns(&p, Expected::Effect),
                None => Res::Err,
            },
            span: self.span(l.syntax().text_range()),
            args: self.type_arg_list(l.base_type_args()),
            default: l.is_default(),
        });
        Item::Effect(EffectItem {
            type_params,
            ops: ops.to_vec(),
            members,
            label,
            // Outside the bundled sources, `@intrinsic` is E2201 and the effect stays a user
            // effect.
            is_intrinsic: e.has_annotation("intrinsic") && self.r.is_core(self.module),
        })
    }

    fn handler_item(&mut self, def: DefId, ctor: Option<DefId>, h: &ast::HandlerDecl) -> Item {
        let outer = self.handler.replace((def, ctor));
        let names = self.inner_names.len();
        let inner = self.r.inner_handlers(def);
        self.inner_names
            .extend(inner.iter().map(|(d, _)| self.r.def(*d).name.clone()));
        let item = self.handler_body(def, ctor, h);
        self.inner_names.truncate(names);
        self.handler = outer;
        item
    }

    /// The inner handlers of `def` (`[effect.handler]`), lowered as handlers of their own. Each
    /// one implements the handler member of the outer handler's effect with its name.
    fn inner_handlers(
        &mut self,
        def: DefId,
        effects: &[EffectRef],
        h: &ast::HandlerDecl,
    ) -> Vec<DefId> {
        let inner = self.r.inner_handlers(def);
        let mut defs = Vec::new();
        let decls = h.inner_handlers().filter(|it| it.name().is_some());
        for ((inner_def, inner_ctor), decl) in inner.into_iter().zip(decls) {
            let name = self.r.def(inner_def).name.clone();
            let range = self
                .r
                .def(inner_def)
                .span
                .map_or(TextRange::default(), |s| s.range);
            let member = self.member_of_effects(effects, &name, range, true);
            let mut item = match self.handler_item(inner_def, inner_ctor, &decl) {
                Item::Handler(item) => item,
                _ => unreachable!("a handler item"),
            };
            item.member = Some(member);
            self.r.insert_item(inner_def, Item::Handler(item));
            defs.push(inner_def);
        }
        defs
    }

    /// The type parameters of a handler (`[effect.handler]`) are in scope in its fields, the
    /// effects after `:`, init, release, the operation bodies and the inner handlers. Those of an
    /// inner handler are kept so that their names resolve; type inference reports them (E1213).
    fn handler_body(&mut self, def: DefId, ctor: Option<DefId>, h: &ast::HandlerDecl) -> Item {
        let type_params = self.type_params(def, h.type_param_list());
        let item = self.handler_body_in(def, ctor, h, type_params);
        self.pop_type_params();
        item
    }

    fn handler_body_in(
        &mut self,
        def: DefId,
        ctor: Option<DefId>,
        h: &ast::HandlerDecl,
        type_params: Vec<TypeParam>,
    ) -> Item {
        let fields = self.named_fields(h.field_list());
        if let Some(ctor) = ctor {
            // The constructor of a handler without fields is a value (`ConsoleLogger`).
            let ctor_fields = match &fields {
                Some(fields) => Fields::Named(fields.clone()),
                None => Fields::None,
            };
            self.r.insert_item(
                ctor,
                Item::Ctor(CtorItem {
                    owner: def,
                    fields: ctor_fields,
                }),
            );
        }
        let mut fields = fields.unwrap_or_default();
        let effects = self.handler_effects(h.effects());
        let is_intrinsic = h.declares_only()
            && h.syntax()
                .children()
                .filter_map(ast::Annotation::cast)
                .any(|a| a.name().is_some_and(|n| n.text() == "intrinsic"))
            && self.r.is_core(self.module);
        // `init` runs on entering `with` and builds the handler value; there is no `self` yet. Its
        // parameters are in scope in its body only.
        // The init of a built-in handler declares only its parameters (`[task.background]`).
        let (init_params, init) = match (h.init(), h.init_params()) {
            (Some(b), params) => {
                self.push_scope();
                let params = self.params(params, LocalKind::Param);
                let init_of = ctor.map(|ctor| InitOf {
                    handler: def,
                    ctor,
                    inner: self
                        .r
                        .def(def)
                        .parent
                        .is_some_and(|p| self.r.def(p).kind == DefKind::Handler),
                    fields: fields.iter().map(|f| f.name.clone()).collect(),
                });
                let outer = std::mem::replace(&mut self.init_of, init_of);
                let body = self.block(&b);
                self.init_of = outer;
                self.pop_scope();
                (params, Some(body))
            }
            (None, Some(params)) => {
                self.push_scope();
                let params = self.params(Some(params), LocalKind::Param);
                self.pop_scope();
                (params, None)
            }
            // The fields of a built-in handler are the parameters of its init, which the
            // runtime implements (`Queue(capacity: 100)`, `[task.queue]`); it has no value to
            // hold them in.
            (None, None) if is_intrinsic => {
                let params = std::mem::take(&mut fields)
                    .into_iter()
                    .map(|f| Param {
                        local: self.alloc_local(f.name, LocalKind::Param, f.span.range),
                        ty: Some(f.ty),
                        borrowed: None,
                    })
                    .collect();
                (params, None)
            }
            (None, None) => (Vec::new(), None),
        };
        let release = h.release().map(|b| {
            self.push_scope();
            let local = self.implicit_self(def);
            let body = self.block(&b);
            self.pop_scope();
            (local, body)
        });
        let ops = h
            .fns()
            .filter_map(|f| {
                let token = f.name()?;
                let (name, range) = token_name(&token);
                let op = self.member_of_effects(&effects, &name, range, false);
                // `@intrinsic` is for top-level fns only. The parser does not allow annotations
                // inside bodies today, but check anyway.
                self.r.check_intrinsic(self.module, f.syntax(), false);
                self.push_scope();
                let self_local = self.implicit_self(def);
                self.push_scope();
                let params = self.params(f.param_list(), LocalKind::Param);
                let body = f.body().map(|b| self.block(&b));
                self.pop_scope();
                self.pop_scope();
                Some(HandlerOp {
                    name: Label {
                        name,
                        span: self.span(range),
                    },
                    op,
                    is_suspend: f.is_suspend(),
                    self_local,
                    params,
                    body,
                })
            })
            .collect();
        let inner = self.inner_handlers(def, &effects, h);
        Item::Handler(HandlerItem {
            type_params,
            effects,
            fields,
            ctor,
            init,
            init_params,
            release,
            ops,
            inner,
            member: None,
            is_intrinsic,
        })
    }

    /// The implicit `self` of handler operations and `release`, located at the handler's name.
    fn implicit_self(&mut self, handler: DefId) -> LocalId {
        let range = self
            .r
            .def(handler)
            .span
            .map_or(TextRange::default(), |s| s.range);
        let local = self.alloc_local("self".into(), LocalKind::SelfParam, range);
        self.locals
            .last_mut()
            .expect("a scope is pushed")
            .insert("self".into(), local);
        local
    }

    /// The effects after the `:` of a handler or a handler member (`[grammar.decl]`).
    fn handler_effects(
        &mut self,
        effects: impl Iterator<Item = ast::HandlerEffect>,
    ) -> Vec<EffectRef> {
        effects
            .map(|e| {
                let res = match e.path() {
                    Some(p) => self.resolve_type_ns(&p, Expected::Effect),
                    None => Res::Err,
                };
                let args = self.type_arg_list(e.type_args());
                let span = self.span(e.syntax().text_range());
                EffectRef { res, args, span }
            })
            .collect()
    }

    /// The operation (or, with `members`, the handler member) `name` that a handler implements,
    /// looked up in each of the effects it lists, in order (`[effect.handler]`). Two of them
    /// having the name is reported by type inference; the first is taken here.
    ///
    /// Not found is E2102, naming every listed effect. Nothing is reported when one of the
    /// effects did not resolve, or is a label: a handler of a label is E4003, and the operations
    /// of a label are not looked up.
    fn member_of_effects(
        &mut self,
        effects: &[EffectRef],
        name: &SmolStr,
        range: TextRange,
        members: bool,
    ) -> Res {
        let wanted = |r: &Resolver, d: DefId| {
            let kind = r.def(d).kind;
            if members {
                kind == DefKind::Member
            } else {
                kind != DefKind::Member
            }
        };
        let mut owners = Vec::new();
        for effect in effects {
            let Res::Def(e) = effect.res else {
                return Res::Err;
            };
            if self.r.is_label(e) {
                return Res::Err;
            }
            owners.push(e);
        }
        for &e in &owners {
            if let Some(d) = self.r.member(e, name).filter(|&d| wanted(self.r, d)) {
                self.refer(range, RefTarget::Def(d));
                return Res::Def(d);
            }
        }
        if owners.is_empty() {
            return Res::Err;
        }
        let names: Vec<SmolStr> = owners
            .iter()
            .flat_map(|&e| self.r.members_of(e))
            .filter(|&d| wanted(self.r, d))
            .map(|d| self.r.def(d).name.clone())
            .collect();
        let suggestion = best_match(name, &names);
        let owner = owners
            .iter()
            .map(|&e| self.r.def(e).name.to_string())
            .collect::<Vec<_>>()
            .join(" + ");
        self.error(
            range,
            DiagnosticKind::UndefinedMember {
                name: name.clone(),
                owner: owner.into(),
                owner_kind: "effect",
                suggestion,
            },
        );
        Res::Err
    }

    /// Reports if not found.
    fn member_of(&mut self, owner: Res, name: &SmolStr, range: TextRange) -> Res {
        let Res::Def(owner) = owner else {
            return Res::Err;
        };
        if let Some(def) = self.r.member(owner, name) {
            self.refer(range, RefTarget::Def(def));
            return Res::Def(def);
        }
        let data = &self.r.def(owner);
        let (owner_name, owner_kind) = (data.name.clone(), data.kind.describe());
        let suggestion = best_match(name, &self.r.member_names(owner));
        self.error(
            range,
            DiagnosticKind::UndefinedMember {
                name: name.clone(),
                owner: owner_name,
                owner_kind,
                suggestion,
            },
        );
        Res::Err
    }

    fn trait_item(&mut self, fns: &[DefId], t: &ast::TraitDecl) -> Item {
        let supertraits = t
            .supertraits()
            .map(|p| {
                let res = self.resolve_type_ns(&p, Expected::Trait);
                (res, self.span(p.syntax().text_range()))
            })
            .collect();
        self.self_type = true;
        for f in t.fns() {
            let Some(name) = f.name() else { continue };
            let Some(&def) = fns
                .iter()
                .find(|&&d| self.r.def(d).span.map(|s| s.range) == Some(name.text_range()))
            else {
                continue;
            };
            self.r.check_intrinsic(self.module, f.syntax(), false);
            let item = self.fn_item(def, &f);
            self.r.insert_item(def, Item::TraitFn(item));
        }
        self.self_type = false;
        Item::Trait(TraitItem {
            supertraits,
            fns: fns.to_vec(),
        })
    }

    fn impl_item(&mut self, def: DefId, i: &ast::ImplDecl) -> Item {
        let trait_ref = match i.trait_ref() {
            Some(p) => self.resolve_type_ns(&p, Expected::Trait),
            None => Res::Err,
        };
        // `impl Map[K: Eq, V: Eq]: Eq` declares `K` and `V` for the impl (spec 10.2).
        let (type_params, malformed) = self.type_params_in(def, i.type_params(), true);
        if malformed && let Some(node) = i.self_ty() {
            let head = node
                .text()
                .to_string()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            self.error(
                node.text_range(),
                DiagnosticKind::ImplHead { head: head.into() },
            );
        }
        // The type is the name applied to its own type parameters (`Map[K, V]`).
        let self_ty = i.self_ty().map(|node| {
            let res = match i.self_path() {
                Some(path) => self.resolve_type_ns(&path, Expected::Type),
                None => Res::Err,
            };
            let args = type_params
                .iter()
                .map(|param| {
                    let range = self
                        .r
                        .def(param.def)
                        .span
                        .map_or(node.text_range(), |s| s.range);
                    let kind = TypeKind::Path {
                        res: Res::Def(param.def),
                        args: Vec::new(),
                    };
                    self.alloc_type(kind, range)
                })
                .collect();
            self.alloc_type(TypeKind::Path { res, args }, node.text_range())
        });
        self.self_type = true;
        let mut seen: FxHashMap<SmolStr, TextRange> = FxHashMap::default();
        let fns = i
            .fns()
            .filter_map(|f| {
                let token = f.name()?;
                let (name, range) = token_name(&token);
                // Like the functions of a trait, an impl's functions cannot repeat a name
                // (E2103, spec 2.3).
                if let Some(&first) = seen.get(&name) {
                    self.error(
                        range,
                        DiagnosticKind::Duplicate {
                            name: name.clone(),
                            first: Some(first),
                        },
                    );
                } else {
                    seen.insert(name.clone(), range);
                }
                let trait_fn = self.member_of(trait_ref, &name, range);
                self.r.check_intrinsic(self.module, f.syntax(), false);
                let fn_def =
                    self.r
                        .alloc_def(self.module, name.clone(), range, DefKind::ImplFn, Some(def));
                self.push_scope();
                let params = self.params(f.param_list(), LocalKind::Param);
                let body = f.body().map(|b| self.block(&b));
                self.pop_scope();
                Some(ImplFn {
                    def: fn_def,
                    name: Label {
                        name,
                        span: self.span(range),
                    },
                    trait_fn,
                    params,
                    body,
                })
            })
            .collect();
        self.self_type = false;
        self.pop_type_params();
        Item::Impl(ImplItem {
            trait_ref,
            type_params,
            self_ty,
            malformed_head: malformed,
            fns,
        })
    }
}

/// The arguments of an fn's `@external(target, "a", "b")` or `@export(target, "a", "b")`
/// (`attr`). Skips annotations whose first argument is not a name (`@external("x")`), and
/// non-string remaining arguments (`@external(js, 1)`, `@external(js, k: "v")`). The parser
/// reports them (E1301, E1301).
fn attrs(f: &ast::FnDecl, attr: &str, module: ModuleId) -> Vec<ExternalAttr> {
    f.annotations()
        .filter(|a| a.name().is_some_and(|n| n.text() == attr))
        .filter_map(|a| {
            let mut args = a.args();
            let first = args.next()?;
            let target = first
                .children_with_tokens()
                .filter_map(|e| e.into_token())
                .find(|t| t.kind() == LOWER_NAME)
                .filter(|_| first.children().next().is_none())?;
            let args = args
                .filter(|arg| {
                    !arg.children_with_tokens()
                        .any(|e| e.kind() == LOWER_NAME || e.kind() == COLON)
                })
                .filter_map(|arg| {
                    let lit = arg.children().find(|n| n.kind() == STRING)?;
                    Some(
                        lit.descendants_with_tokens()
                            .filter_map(|e| e.into_token())
                            .filter(|t| t.kind() == STRING_TEXT)
                            .map(|t| string_text(&t))
                            .collect(),
                    )
                })
                .collect();
            Some(ExternalAttr {
                target: target.text().into(),
                args,
                span: Span {
                    module,
                    range: a.syntax().text_range(),
                },
            })
        })
        .collect()
}
