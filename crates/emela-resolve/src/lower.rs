//! Lowers declaration contents to HIR. Holds the local scopes (spec 6.3, 6.5, 7.3, 8.4) and the
//! type parameter scopes.

use emela_syntax::SyntaxKind::{self, *};
use emela_syntax::ast::{self, AstNode, HasAttrs};
use emela_syntax::{SyntaxNode, SyntaxToken};
use rowan::TextRange;
use rustc_hash::FxHashMap;
use smol_str::SmolStr;

use crate::diagnostic::{DiagnosticKind, Expected};
use crate::hir::*;
use crate::resolve::{DotLeft, ModRef, Ns, Pending, Resolver, child_token, token_name};
use crate::source::ModuleId;
use crate::suggest::best_match;

pub(crate) fn lower_item(resolver: &mut Resolver<'_>, pending: Pending) {
    let mut lower = Lower {
        r: resolver,
        module: pending.module,
        locals: Vec::new(),
        type_params: Vec::new(),
        self_type: false,
        type_arg: false,
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
                .handlers()
                .map(|p| {
                    let res = lower.resolve_type_ns(&p, Expected::HandlerOrLayer);
                    (res, lower.span(p.syntax().text_range()))
                })
                .collect();
            Item::Layer(LayerItem { members })
        }
        ast::Item::Trait(t) => lower.trait_item(&pending.children, t),
        ast::Item::Impl(i) => lower.impl_item(pending.def, i),
    };
    lower.r.program.items.insert(pending.def, item);
}

struct Lower<'r, 'a> {
    r: &'r mut Resolver<'a>,
    module: ModuleId,
    /// Innermost last. Rebinding overwrites the name in the same scope.
    locals: Vec<FxHashMap<SmolStr, LocalId>>,
    type_params: Vec<FxHashMap<SmolStr, DefId>>,
    /// Whether `Self` may be written (inside traits and impls).
    self_type: bool,
    /// Set while looking up a type in a type argument position of a named type. Effect names may
    /// appear there too (set type arguments, `Handler[NotFound, Db]` in spec 9.2); whether an
    /// effect is allowed in that position is checked by type inference.
    type_arg: bool,
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
        let span = self.span(range);
        self.r.program.refs.push(NameRef { span, target });
    }

    fn alloc_expr(&mut self, kind: ExprKind, range: TextRange) -> ExprId {
        let span = self.span(range);
        self.r.program.exprs.alloc(Expr { kind, span })
    }

    fn alloc_pat(&mut self, kind: PatKind, range: TextRange) -> PatId {
        let span = self.span(range);
        self.r.program.pats.alloc(Pat { kind, span })
    }

    fn alloc_type(&mut self, kind: TypeKind, range: TextRange) -> TypeId {
        let span = Some(self.span(range));
        self.r.program.types.alloc(TypeRef { kind, span })
    }

    fn alloc_local(&mut self, name: SmolStr, kind: LocalKind, range: TextRange) -> LocalId {
        let span = self.span(range);
        self.r.program.locals.alloc(LocalData { name, kind, span })
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
            let first = self.r.program.locals[first].span.range;
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
    /// `pop_type_params`.
    fn type_params(&mut self, owner: DefId, list: Option<SyntaxNode>) -> Vec<TypeParam> {
        self.type_params_in(owner, list, false).0
    }

    /// Reads the type parameters like [`Self::type_params`]. In an impl head (`impl_head`), a
    /// repeated name or the name of a type is not a type parameter but a malformed head (E0328,
    /// spec 10.2), so neither E0213 nor W0201 is reported; the second value tells whether the head
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
        FnItem {
            sig,
            body,
            is_external: f.has_annotation("external"),
            externals: external_attrs(f),
            is_intrinsic: f.has_annotation("intrinsic"),
        }
    }

    /// `-> T fails E use R`. `node` is a FN_DECL or OP_SIG.
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
            .find(|n| n.kind() == USE_CLAUSE)
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
            params.push(Param { local, ty });
        }
        self.bind_all(binder);
        params
    }

    /// Parameters of a lambda. A pattern in place of a name becomes a hidden parameter
    /// ([`LocalKind::PatParam`]) and a binding `pattern = parameter` to put at the head of the
    /// body (spec 6.1), so type inference checks it as a binding (E0317). Names bound by all
    /// parameters share one binder (E0220).
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
                params.push(Param { local, ty });
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
                params.push(Param { local, ty });
            } else if let Some(node) = param.pattern() {
                let range = node.text_range();
                let pat = self.pat(&node, &mut binder);
                let local =
                    self.alloc_local(node.text().to_string().into(), LocalKind::PatParam, range);
                let value = self.alloc_expr(ExprKind::Path(Res::Local(local)), range);
                params.push(Param { local, ty });
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
        let derives = self.derives(t.derive());
        self.pop_type_params();
        if let Some(ctor) = ctor {
            self.r.program.items.insert(
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
        })
    }

    fn enum_item(&mut self, def: DefId, variants: &[DefId], e: &ast::EnumDecl) -> Item {
        let type_params = self.type_params(
            def,
            e.syntax().children().find(|n| n.kind() == TYPE_PARAM_LIST),
        );
        // Variants with unreadable names were skipped during collection, so match up only the
        // named ones, in order.
        let syntax_variants: Vec<_> = e.variants().filter(|v| v.name().is_some()).collect();
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
                .program
                .items
                .insert(variant, Item::Ctor(CtorItem { owner: def, fields }));
        }
        let derives = self.derives(e.derive());
        self.pop_type_params();
        Item::Enum(EnumItem {
            type_params,
            variants: variants.to_vec(),
            derives,
        })
    }

    fn effect_item(&mut self, def: DefId, ops: &[DefId], e: &ast::EffectDecl) -> Item {
        // An effect's type parameters (spec 8.1) are usable in its operation signatures.
        let type_params = self.type_params(def, e.type_params());
        // Duplicate operations were skipped during collection, so match by name.
        for op in e.ops() {
            let Some(name) = op.name() else { continue };
            let Some(&def) = ops.iter().find(|&&d| {
                let data = &self.r.program.defs[d];
                data.name == name.text() && data.span.map(|s| s.range) == Some(name.text_range())
            }) else {
                continue;
            };
            // An operation's own type parameters (`fork[A]`, spec 8.1, 11.2). Only built-in
            // effects may have them; type inference reports E0426 otherwise.
            let op_params = self.type_params(def, op.type_params());
            self.push_scope();
            let params = self.params(op.param_list(), LocalKind::Param);
            let sig = self.signature(op.syntax(), op_params, params, op.is_suspend());
            self.pop_type_params();
            self.pop_scope();
            self.r.program.items.insert(def, Item::Op(OpItem { sig }));
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
            label,
            // Outside the bundled sources, `@intrinsic` is E0224 and the effect stays a user
            // effect.
            is_intrinsic: e.has_annotation("intrinsic") && self.r.modules.is_core(self.module),
        })
    }

    fn handler_item(&mut self, def: DefId, ctor: Option<DefId>, h: &ast::HandlerDecl) -> Item {
        let fields = self.named_fields(h.field_list());
        if let Some(ctor) = ctor {
            // The constructor of a handler without fields is a value (`ConsoleLogger`).
            let ctor_fields = match &fields {
                Some(fields) => Fields::Named(fields.clone()),
                None => Fields::None,
            };
            self.r.program.items.insert(
                ctor,
                Item::Ctor(CtorItem {
                    owner: def,
                    fields: ctor_fields,
                }),
            );
        }
        let fields = fields.unwrap_or_default();
        let effect = match h.effect() {
            Some(p) => self.resolve_type_ns(&p, Expected::Effect),
            None => Res::Err,
        };
        let effect_args = self.type_arg_list(h.effect_type_args());
        // `init` runs on entering `with` and builds the handler value; there is no `self` yet.
        let init = h.init().map(|b| self.block(&b));
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
                // A handler of a label is E0433; its operations are not looked up.
                let op = match effect {
                    Res::Def(e) if self.r.labels.contains(&e) => Res::Err,
                    _ => self.member_of(effect, &name, range),
                };
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
        Item::Handler(HandlerItem {
            effect,
            effect_args,
            fields,
            ctor,
            init,
            release,
            ops,
        })
    }

    /// The implicit `self` of handler operations and `release`, located at the handler's name.
    fn implicit_self(&mut self, handler: DefId) -> LocalId {
        let range = self.r.program.defs[handler]
            .span
            .map_or(TextRange::default(), |s| s.range);
        let local = self.alloc_local("self".into(), LocalKind::SelfParam, range);
        self.locals
            .last_mut()
            .expect("a scope is pushed")
            .insert("self".into(), local);
        local
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
        let data = &self.r.program.defs[owner];
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
            let Some(&def) = fns.iter().find(|&&d| {
                self.r.program.defs[d].span.map(|s| s.range) == Some(name.text_range())
            }) else {
                continue;
            };
            self.r.check_intrinsic(self.module, f.syntax(), false);
            let item = self.fn_item(def, &f);
            self.r.program.items.insert(def, Item::TraitFn(item));
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
                    let range = self.r.program.defs[param.def]
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
                // (E0213, spec 2.3).
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

    fn lower_type(&mut self, node: &SyntaxNode) -> TypeId {
        let range = node.text_range();
        let type_arg = std::mem::take(&mut self.type_arg);
        let kind = match node.kind() {
            PATH_TYPE => {
                let res = match node.children().find_map(ast::Path::cast) {
                    Some(path) => {
                        self.type_arg = type_arg;
                        let res = self.resolve_type_ns(&path, Expected::Type);
                        self.type_arg = false;
                        res
                    }
                    None => Res::Err,
                };
                let args = node
                    .children()
                    .find(|n| n.kind() == TYPE_ARG_LIST)
                    .map(|list| {
                        list.children()
                            .map(|t| {
                                self.type_arg = true;
                                self.lower_type(&t)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                TypeKind::Path { res, args }
            }
            TYPE_VAR => TypeKind::Path {
                res: self.type_var(node),
                args: Vec::new(),
            },
            SELF_TYPE => {
                if self.self_type {
                    TypeKind::SelfType
                } else {
                    self.error(range, DiagnosticKind::SelfOutside { self_type: true });
                    TypeKind::Missing
                }
            }
            // The parentheses of `(fn(A) -> B)` do not change the type.
            PAREN_TYPE => {
                if let Some(inner) = node.first_child() {
                    return self.lower_type(&inner);
                }
                TypeKind::Missing
            }
            UNIT_TYPE => TypeKind::Unit,
            // A set in a type argument position (spec 9.2). The parser creates these only there.
            ERROR_SET => TypeKind::ErrorSet(ErrorSet {
                items: self.set_node_items(node, Expected::Error),
                span: self.span(range),
            }),
            EFFECT_SET => TypeKind::EffectSet(EffectSet {
                items: self.effect_set_items(node),
                span: self.span(range),
            }),
            TUPLE_TYPE => TypeKind::Tuple(node.children().map(|t| self.lower_type(&t)).collect()),
            FN_TYPE => {
                let params = node
                    .children()
                    .find(|n| n.kind() == PARAM_TYPE_LIST)
                    .map(|list| list.children().map(|t| self.lower_type(&t)).collect())
                    .unwrap_or_default();
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
                    .find(|n| n.kind() == USE_CLAUSE)
                    .map(|c| self.effect_set(&c));
                TypeKind::Fn {
                    params,
                    ret,
                    fails,
                    uses,
                }
            }
            _ => TypeKind::Missing,
        };
        self.alloc_type(kind, range)
    }

    /// Uppercase names in type positions. Only type parameters are looked up.
    fn type_var(&mut self, node: &SyntaxNode) -> Res {
        let Some(token) = child_token(node, UPPER_NAME) else {
            return Res::Err;
        };
        let (name, range) = token_name(&token);
        if let Some(def) = self.lookup_type_param(&name) {
            self.refer(range, RefTarget::Def(def));
            return Res::Def(def);
        }
        let candidates: Vec<SmolStr> = self
            .type_params
            .iter()
            .flat_map(|s| s.keys().cloned())
            .collect();
        let suggestion = best_match(&name, &candidates);
        self.error(
            range,
            DiagnosticKind::UndefinedName {
                name,
                expected: Expected::TypeParam,
                suggestion,
            },
        );
        Res::Err
    }

    /// The ERROR_SET inside a `FAILS_CLAUSE`.
    fn error_set(&mut self, clause: &SyntaxNode) -> ErrorSet {
        let items = self.set_items(clause, Expected::Error);
        ErrorSet {
            items,
            span: self.span(clause.text_range()),
        }
    }

    fn effect_set(&mut self, clause: &SyntaxNode) -> EffectSet {
        let items = match clause.children().find(|n| n.kind() == EFFECT_SET) {
            Some(set) => self.effect_set_items(&set),
            None => Vec::new(),
        };
        EffectSet {
            items,
            span: self.span(clause.text_range()),
        }
    }

    /// Elements of an EFFECT_SET. An effect with type arguments (`State[Int]`, spec 8.1) is a
    /// PATH_TYPE.
    fn effect_set_items(&mut self, set: &SyntaxNode) -> Vec<EffectRef> {
        set.children()
            .filter_map(|n| {
                let (res, args) = match n.kind() {
                    PATH => (
                        self.resolve_type_ns(&ast::Path::cast(n.clone())?, Expected::Effect),
                        Vec::new(),
                    ),
                    PATH_TYPE => {
                        let path = n.children().find_map(ast::Path::cast)?;
                        let res = self.resolve_type_ns(&path, Expected::Effect);
                        let args =
                            self.type_arg_list(n.children().find(|c| c.kind() == TYPE_ARG_LIST));
                        (res, args)
                    }
                    TYPE_VAR => (self.type_var(&n), Vec::new()),
                    _ => return None,
                };
                Some(EffectRef {
                    res,
                    args,
                    span: self.span(n.text_range()),
                })
            })
            .collect()
    }

    /// Types in a TYPE_ARG_LIST. Empty if there is no list.
    fn type_arg_list(&mut self, list: Option<SyntaxNode>) -> Vec<TypeId> {
        let Some(list) = list else {
            return Vec::new();
        };
        list.children().map(|t| self.lower_type(&t)).collect()
    }

    fn set_items(&mut self, clause: &SyntaxNode, expected: Expected) -> Vec<(Res, Span)> {
        let Some(set) = clause
            .children()
            .find(|n| matches!(n.kind(), ERROR_SET | EFFECT_SET))
        else {
            return Vec::new();
        };
        self.set_node_items(&set, expected)
    }

    /// Elements of an ERROR_SET or EFFECT_SET.
    fn set_node_items(&mut self, set: &SyntaxNode, expected: Expected) -> Vec<(Res, Span)> {
        set.children()
            .filter_map(|n| {
                let res = match n.kind() {
                    PATH => self.resolve_type_ns(&ast::Path::cast(n.clone())?, expected),
                    TYPE_VAR => self.type_var(&n),
                    _ => return None,
                };
                Some((res, self.span(n.text_range())))
            })
            .collect()
    }

    /// Looks up a path in the type namespace and checks its kind. A single segment is looked up
    /// as a type parameter, then a module definition, then the Prelude; `A.B` is the pub type of
    /// module `A`.
    fn resolve_type_ns(&mut self, path: &ast::Path, expected: Expected) -> Res {
        let segments: Vec<SyntaxToken> = path.segments().collect();
        let Some((last, prefix)) = segments.split_last() else {
            return Res::Err;
        };
        let (name, range) = token_name(last);
        let def = if prefix.is_empty() {
            let found = self
                .lookup_type_param(&name)
                .or_else(|| self.r.lookup(self.module, Ns::Type, &name))
                .or_else(|| self.r.builtin_eponymous_type(self.module, &name));
            match found {
                Some(def) => def,
                None => {
                    self.undefined_type(&name, range, expected);
                    return Res::Err;
                }
            }
        } else {
            let Some(left) = self.resolve_prefix(prefix) else {
                return Res::Err;
            };
            match self.member_of_left(&left, Ns::Type, &name, range) {
                Some(def) => def,
                None => return Res::Err,
            }
        };
        let kind = self.r.program.defs[def].kind;
        let fits = match expected {
            Expected::Type => {
                matches!(
                    kind,
                    DefKind::Type
                        | DefKind::Enum
                        | DefKind::Error
                        | DefKind::BuiltinType
                        | DefKind::TypeParam
                ) || (self.type_arg && kind == DefKind::Effect)
            }
            Expected::Effect => matches!(kind, DefKind::Effect | DefKind::TypeParam),
            Expected::Error => matches!(kind, DefKind::Error | DefKind::TypeParam),
            Expected::HandlerOrLayer => matches!(kind, DefKind::Handler | DefKind::Layer),
            Expected::WithItem => {
                matches!(kind, DefKind::Handler | DefKind::Layer | DefKind::Effect)
            }
            Expected::Label => kind == DefKind::Effect,
            Expected::HandlerOrEffect => matches!(kind, DefKind::Handler | DefKind::Effect),
            Expected::Trait => kind == DefKind::Trait,
            _ => true,
        };
        // `use X` cannot take type arguments.
        let fits = fits
            && !(expected == Expected::Effect
                && kind == DefKind::TypeParam
                && path_in_use_expr(path));
        if !fits && expected == Expected::Type && kind == DefKind::Effect {
            self.error(range, DiagnosticKind::EffectAsType { name });
            return Res::Err;
        }
        if !fits {
            self.error(
                range,
                DiagnosticKind::WrongKind {
                    name,
                    expected,
                    found: kind.describe(),
                },
            );
            return Res::Err;
        }
        self.refer(range, RefTarget::Def(def));
        Res::Def(def)
    }

    fn undefined_type(&mut self, name: &SmolStr, range: TextRange, expected: Expected) {
        // A module name in a type position (the `Email` of spec 4.4).
        if self.r.scopes[self.module].modules.contains_key(name) {
            self.error(
                range,
                DiagnosticKind::WrongKind {
                    name: name.clone(),
                    expected,
                    found: "module",
                },
            );
            return;
        }
        let mut candidates = self.r.candidates(self.module, Ns::Type);
        candidates.extend(self.type_params.iter().flat_map(|s| s.keys().cloned()));
        let suggestion = best_match(name, &candidates);
        self.error(
            range,
            DiagnosticKind::UndefinedName {
                name: name.clone(),
                expected,
                suggestion,
            },
        );
    }

    /// The `A.B` of `A.B.f`. The first segment is looked up as left of `.`, the rest as traits
    /// inside the module.
    fn resolve_prefix(&mut self, prefix: &[SyntaxToken]) -> Option<DotLeft> {
        let (first, rest) = prefix.split_first()?;
        let (name, range) = token_name(first);
        let mut left = self.r.lookup_dot_left(self.module, &name, range)?;
        match &left {
            DotLeft::Module(ModRef::Source(m)) => self.refer(range, RefTarget::Module(*m)),
            DotLeft::Module(ModRef::Builtin(_)) => {}
            DotLeft::Trait(t) => self.refer(range, RefTarget::Def(*t)),
        }
        for segment in rest {
            let (name, range) = token_name(segment);
            let def = self.member_of_left(&left, Ns::Type, &name, range)?;
            if self.r.program.defs[def].kind != DefKind::Trait {
                let found = self.r.program.defs[def].kind.describe();
                self.error(
                    range,
                    DiagnosticKind::WrongKind {
                        name,
                        expected: Expected::ModuleOrTrait,
                        found,
                    },
                );
                return None;
            }
            self.refer(range, RefTarget::Def(def));
            left = DotLeft::Trait(def);
        }
        Some(left)
    }

    /// A name inside the left of `.`: a pub definition in namespace `ns` for a module, a function
    /// for a trait.
    fn member_of_left(
        &mut self,
        left: &DotLeft,
        ns: Ns,
        name: &SmolStr,
        range: TextRange,
    ) -> Option<DefId> {
        match left {
            DotLeft::Module(m) => self.r.module_member(self.module, m, ns, name, range),
            DotLeft::Trait(t) => {
                let found = if ns == Ns::Value {
                    self.r.member(*t, name)
                } else {
                    None
                };
                if found.is_none() {
                    let data = &self.r.program.defs[*t];
                    let owner = data.name.clone();
                    let suggestion = best_match(name, &self.r.member_names(*t));
                    self.error(
                        range,
                        DiagnosticKind::UndefinedMember {
                            name: name.clone(),
                            owner,
                            owner_kind: "trait",
                            suggestion,
                        },
                    );
                }
                found
            }
        }
    }

    fn opt_expr(&mut self, expr: Option<ast::Expr>, range: TextRange) -> ExprId {
        match expr {
            Some(e) => self.expr(e.syntax()),
            None => self.alloc_expr(ExprKind::Missing, range),
        }
    }

    /// `Missing` if absent.
    fn child_expr(&mut self, node: &SyntaxNode, nth: usize) -> ExprId {
        match node.children().filter(|c| is_expr(c.kind())).nth(nth) {
            Some(c) => self.expr(&c),
            None => self.alloc_expr(ExprKind::Missing, node.text_range()),
        }
    }

    fn block(&mut self, block: &ast::BlockExpr) -> ExprId {
        self.expr(block.syntax())
    }

    fn expr(&mut self, node: &SyntaxNode) -> ExprId {
        let range = node.text_range();
        let kind = match node.kind() {
            LITERAL => match node.first_token() {
                Some(t) if t.kind() == INT => {
                    ExprKind::Lit(Literal::Int(t.text().replace('_', "").into()))
                }
                Some(t) if t.kind() == FLOAT => {
                    ExprKind::Lit(Literal::Float(t.text().replace('_', "").into()))
                }
                _ => ExprKind::Missing,
            },
            STRING => self.string(node),
            NAME_REF => ExprKind::Path(self.name_ref(node)),
            PATH_EXPR => match node.children().find_map(ast::Path::cast) {
                Some(path) => ExprKind::Path(self.ctor_path(&path, false)),
                None => ExprKind::Missing,
            },
            UNIT_EXPR => ExprKind::Unit,
            PAREN_EXPR => {
                return match node.children().find(|c| is_expr(c.kind())) {
                    Some(inner) => self.expr(&inner),
                    None => self.alloc_expr(ExprKind::Missing, range),
                };
            }
            TUPLE_EXPR => ExprKind::Tuple(
                node.children()
                    .filter(|c| is_expr(c.kind()))
                    .map(|c| self.expr(&c))
                    .collect(),
            ),
            LIST_EXPR => {
                let mut elems = Vec::new();
                let mut rest = None;
                for child in node.children() {
                    if child.kind() == REST_EXPR {
                        rest = child
                            .children()
                            .find(|c| is_expr(c.kind()))
                            .map(|c| self.expr(&c));
                    } else if is_expr(child.kind()) {
                        elems.push(self.expr(&child));
                    }
                }
                ExprKind::List { elems, rest }
            }
            CALL_EXPR => self.call(node),
            FIELD_EXPR => self.field(node),
            PREFIX_EXPR => {
                let op = match node.first_token().map(|t| t.kind()) {
                    Some(BANG) => UnaryOp::Not,
                    _ => UnaryOp::Neg,
                };
                let operand = self.child_expr(node, 0);
                ExprKind::Unary { op, operand }
            }
            BIN_EXPR => self.binary(node),
            USE_EXPR => {
                let effect = match node.children().find_map(ast::Path::cast) {
                    Some(path) => self.resolve_type_ns(&path, Expected::Effect),
                    None => Res::Err,
                };
                let args = self.type_arg_list(node.children().find(|n| n.kind() == TYPE_ARG_LIST));
                ExprKind::Use { effect, args }
            }
            FAIL_EXPR => {
                // The `e` of `fail e` is an ordinary expression (a value of the error type); type
                // inference checks its type (spec 7.2).
                ExprKind::Fail(self.child_expr(node, 0))
            }
            ASSERT_EXPR => ExprKind::Assert(self.child_expr(node, 0)),
            BLOCK_EXPR => {
                self.push_scope();
                let block = self.block_body(node);
                self.pop_scope();
                ExprKind::Block(block)
            }
            IF_EXPR => {
                let mut children = node.children().filter(|c| is_expr(c.kind()));
                let cond = children.next();
                let then_branch = children.next();
                let else_branch = children.next();
                let cond = match cond {
                    Some(c) => self.expr(&c),
                    None => self.alloc_expr(ExprKind::Missing, range),
                };
                let then_branch = match then_branch {
                    Some(c) => self.expr(&c),
                    None => self.alloc_expr(ExprKind::Missing, range),
                };
                let else_branch = else_branch.map(|c| self.expr(&c));
                ExprKind::If {
                    cond,
                    then_branch,
                    else_branch,
                }
            }
            MATCH_EXPR => {
                let scrutinee = self.child_expr(node, 0);
                let arms = self.arms(node, false);
                ExprKind::Match { scrutinee, arms }
            }
            ESCAPE_EXPR => {
                let expr = self.child_expr(node, 0);
                let arms = self.arms(node, true);
                ExprKind::Escape { expr, arms }
            }
            WITH_EXPR => {
                let handlers = node
                    .children()
                    .filter_map(|c| {
                        if c.kind() == WITH_BY {
                            let mut paths = c.children().filter_map(ast::Path::cast);
                            let label = paths.next()?;
                            let res = self.resolve_type_ns(&label, Expected::Label);
                            let by = paths.next().map(|p| {
                                let res = self.resolve_type_ns(&p, Expected::HandlerOrEffect);
                                (res, self.span(p.syntax().text_range()))
                            });
                            let by_args = self
                                .type_arg_list(c.children().find(|n| n.kind() == TYPE_ARG_LIST));
                            return Some(WithItem {
                                res,
                                span: self.span(c.text_range()),
                                by: Some(by.unwrap_or((Res::Err, self.span(c.text_range())))),
                                by_args,
                            });
                        }
                        let p = ast::Path::cast(c)?;
                        let res = self.resolve_type_ns(&p, Expected::WithItem);
                        Some(WithItem {
                            res,
                            span: self.span(p.syntax().text_range()),
                            by: None,
                            by_args: Vec::new(),
                        })
                    })
                    .collect();
                let body = self.child_expr(node, 0);
                ExprKind::With { handlers, body }
            }
            LAMBDA_EXPR => {
                self.push_scope();
                let (params, lets) =
                    self.lambda_params(node.children().find_map(ast::ParamList::cast));
                let body = self.child_expr(node, 0);
                self.pop_scope();
                // `fn(p) { body }` is `fn(t) { p = t; body }` (spec 6.1).
                if !lets.is_empty()
                    && let ExprKind::Block(block) = &mut self.r.program.exprs[body].kind
                {
                    block.stmts.splice(0..0, lets);
                }
                ExprKind::Lambda { params, body }
            }
            _ => ExprKind::Missing,
        };
        self.alloc_expr(kind, range)
    }

    fn block_body(&mut self, node: &SyntaxNode) -> Block {
        let mut stmts = Vec::new();
        let children: Vec<SyntaxNode> = node
            .children()
            .filter(|c| c.kind() == BINDING || is_expr(c.kind()))
            .collect();
        let mut tail = None;
        for (i, child) in children.iter().enumerate() {
            let last = i + 1 == children.len();
            if child.kind() == BINDING {
                stmts.push(self.binding(child));
            } else {
                let e = self.expr(child);
                if last {
                    tail = Some(e);
                } else {
                    stmts.push(Stmt::Expr(e));
                }
            }
        }
        Block { stmts, tail }
    }

    /// `pat: ty = value`. The right side is resolved first and the pattern's names are bound
    /// after, so `count = count + 1` works.
    fn binding(&mut self, node: &SyntaxNode) -> Stmt {
        let binding = ast::Binding::cast(node.clone()).expect("BINDING");
        let ty = binding.ty().map(|t| self.lower_type(&t));
        let value = self.opt_expr(binding.value(), node.text_range());
        let mut binder = Binder::default();
        let pat = match binding.lhs() {
            Some(lhs) => self.expr_pat(lhs.syntax(), &mut binder),
            None => self.alloc_pat(PatKind::Missing, node.text_range()),
        };
        self.bind_all(binder);
        Stmt::Let { pat, ty, value }
    }

    /// Arms of match and escape. Each arm pushes a scope; pattern names are visible in the guard
    /// and body.
    fn arms(&mut self, node: &SyntaxNode, escape: bool) -> Vec<Arm> {
        let Some(list) = node.children().find(|c| c.kind() == MATCH_ARM_LIST) else {
            return Vec::new();
        };
        list.children()
            .filter(|c| c.kind() == MATCH_ARM)
            .map(|arm| {
                self.push_scope();
                let mut binder = Binder::default();
                let pat = match arm.children().find(|c| is_pat(c.kind())) {
                    Some(p) => {
                        let pat = self.pat(&p, &mut binder);
                        if escape {
                            self.check_escape_pat(pat);
                        }
                        pat
                    }
                    None => self.alloc_pat(PatKind::Missing, arm.text_range()),
                };
                for (_, local) in &binder.names {
                    self.r.program.locals[*local].kind = LocalKind::ArmBinding;
                }
                self.bind_all(binder);
                let guard = arm
                    .children()
                    .find(|c| c.kind() == MATCH_GUARD)
                    .map(|g| self.child_expr(&g, 0));
                let body = self.child_expr(&arm, 0);
                self.pop_scope();
                Arm { pat, guard, body }
            })
            .collect()
    }

    fn string(&mut self, node: &SyntaxNode) -> ExprKind {
        let mut parts: Vec<StrPart> = Vec::new();
        for element in node.children_with_tokens() {
            match element {
                rowan::NodeOrToken::Token(t) if t.kind() == STRING_TEXT => {
                    let text = string_text(&t);
                    match parts.last_mut() {
                        Some(StrPart::Text(prev)) => prev.push_str(&text),
                        _ => parts.push(StrPart::Text(text)),
                    }
                }
                rowan::NodeOrToken::Node(n) if n.kind() == INTERP => {
                    let e = self.child_expr(&n, 0);
                    parts.push(StrPart::Interp(e));
                }
                _ => {}
            }
        }
        match parts.as_slice() {
            [] => ExprKind::Lit(Literal::String(String::new())),
            [StrPart::Text(text)] => ExprKind::Lit(Literal::String(text.clone())),
            _ => ExprKind::Interpolated(parts),
        }
    }

    /// Lowercase and uppercase names in expressions, and `self`.
    fn name_ref(&mut self, node: &SyntaxNode) -> Res {
        let Some(token) = node.first_token() else {
            return Res::Err;
        };
        let (name, range) = token_name(&token);
        self.value(&name, range, token.kind())
    }

    fn value(&mut self, name: &SmolStr, range: TextRange, kind: SyntaxKind) -> Res {
        if kind == SELF_KW {
            return match self.lookup_local("self") {
                Some(local) => {
                    self.refer(range, RefTarget::Local(local));
                    Res::Local(local)
                }
                None => {
                    self.error(range, DiagnosticKind::SelfOutside { self_type: false });
                    Res::Err
                }
            };
        }
        if let Some(local) = self.lookup_local(name) {
            self.refer(range, RefTarget::Local(local));
            return Res::Local(local);
        }
        if let Some(def) = self.r.lookup(self.module, Ns::Value, name) {
            self.refer(range, RefTarget::Def(def));
            return Res::Def(def);
        }
        // Uppercase-name type parameters are usable only in type positions (spec 2.3).
        if let Some(def) = self.lookup_type_param(name) {
            let found = self.r.program.defs[def].kind.describe();
            self.error(
                range,
                DiagnosticKind::WrongKind {
                    name: name.clone(),
                    expected: Expected::Value,
                    found,
                },
            );
            return Res::Err;
        }
        let mut candidates: Vec<SmolStr> =
            self.locals.iter().flat_map(|s| s.keys().cloned()).collect();
        candidates.extend(self.r.candidates(self.module, Ns::Value));
        let suggestion = best_match(name, &candidates);
        self.error(
            range,
            DiagnosticKind::UndefinedName {
                name: name.clone(),
                expected: Expected::Value,
                suggestion,
            },
        );
        Res::Err
    }

    /// A type-name path in an expression or pattern; looks up a constructor. `Email.Email` is the
    /// module's constructor. Using an opaque type's constructor outside its defining module is
    /// reported.
    fn ctor_path(&mut self, path: &ast::Path, in_pattern: bool) -> Res {
        let segments: Vec<SyntaxToken> = path.segments().collect();
        let Some((last, prefix)) = segments.split_last() else {
            return Res::Err;
        };
        let (name, range) = token_name(last);
        let def = if prefix.is_empty() {
            match self.r.lookup(self.module, Ns::Ctor, &name) {
                Some(def) => def,
                None => {
                    self.undefined_ctor(&name, range);
                    return Res::Err;
                }
            }
        } else {
            let Some(left) = self.resolve_prefix(prefix) else {
                return Res::Err;
            };
            match self.member_of_left(&left, Ns::Ctor, &name, range) {
                Some(def) => def,
                None => return Res::Err,
            }
        };
        let data = &self.r.program.defs[def];
        if data.is_opaque && data.module != DefModule::Source(self.module) {
            let module = self.r.def_module_name(def);
            let kind = if in_pattern {
                DiagnosticKind::OpaquePattern { name, module }
            } else {
                DiagnosticKind::OpaqueConstruction { name, module }
            };
            self.error(range, kind);
        }
        self.refer(range, RefTarget::Def(def));
        Res::Def(def)
    }

    fn undefined_ctor(&mut self, name: &SmolStr, range: TextRange) {
        // A type, effect or module name written as a value.
        let found = if let Some(def) = self.r.lookup(self.module, Ns::Type, name) {
            Some(self.r.program.defs[def].kind.describe())
        } else if self.lookup_type_param(name).is_some() {
            Some(DefKind::TypeParam.describe())
        } else if self.r.scopes[self.module].modules.contains_key(name) {
            Some("module")
        } else {
            None
        };
        if let Some(found) = found {
            self.error(
                range,
                DiagnosticKind::WrongKind {
                    name: name.clone(),
                    expected: Expected::Constructor,
                    found,
                },
            );
            return;
        }
        let candidates = self.r.candidates(self.module, Ns::Ctor);
        let suggestion = best_match(name, &candidates);
        self.error(
            range,
            DiagnosticKind::UndefinedName {
                name: name.clone(),
                expected: Expected::Constructor,
                suggestion,
            },
        );
    }

    fn call(&mut self, node: &SyntaxNode) -> ExprKind {
        let callee = self.child_expr(node, 0);
        let mut args = Vec::new();
        let mut spread = None;
        if let Some(list) = node.children().find(|c| c.kind() == ARG_LIST) {
            for arg in list.children() {
                if arg.kind() == SPREAD_ARG {
                    // A `..` without an expression is already reported by the syntax checks.
                    if let Some(e) = arg.children().find(|c| is_expr(c.kind())) {
                        spread = Some(self.expr(&e));
                    }
                    continue;
                }
                if arg.kind() == NAMED_ARG {
                    let Some(token) = child_token(&arg, LOWER_NAME) else {
                        continue;
                    };
                    let (name, range) = token_name(&token);
                    let label = Label {
                        name: name.clone(),
                        span: self.span(range),
                    };
                    // `NotFound(id:)` is short for `id: id`.
                    let (value, punned) = match arg.children().find(|c| is_expr(c.kind())) {
                        Some(e) => (self.expr(&e), false),
                        None => {
                            let res = self.value(&name, range, LOWER_NAME);
                            (self.alloc_expr(ExprKind::Path(res), range), true)
                        }
                    };
                    args.push(Arg {
                        label: Some(label),
                        value,
                        punned,
                    });
                } else if is_expr(arg.kind()) {
                    let value = self.expr(&arg);
                    args.push(Arg {
                        label: None,
                        value,
                        punned: false,
                    });
                }
            }
        }
        match spread {
            Some(base) => self.update(callee, args, base),
            None => ExprKind::Call { callee, args },
        }
    }

    /// A record update `C(f: e, ..base)` (spec 5.3). `C` must have a single constructor (a type,
    /// an error, or a handler with fields), and changed fields are given by name.
    fn update(&mut self, ctor: ExprId, args: Vec<Arg>, base: ExprId) -> ExprKind {
        let Expr { kind, span } = &self.r.program.exprs[ctor];
        let range = span.range;
        let target = match kind {
            ExprKind::Path(Res::Def(def)) => Some(*def),
            // Unresolved names are already reported.
            ExprKind::Path(Res::Err) | ExprKind::Missing => None,
            ExprKind::Path(Res::Local(local)) => {
                let name = self.r.program.locals[*local].name.clone();
                self.error(
                    range,
                    DiagnosticKind::InvalidUpdateTarget {
                        name: Some(name),
                        found: "local variable",
                    },
                );
                None
            }
            _ => {
                self.error(
                    range,
                    DiagnosticKind::InvalidUpdateTarget {
                        name: None,
                        found: "expression",
                    },
                );
                None
            }
        };
        if let Some(def) = target {
            let data = &self.r.program.defs[def];
            let single = match data.kind {
                DefKind::Error => true,
                DefKind::Ctor => data.parent.is_some_and(|p| {
                    matches!(
                        self.r.program.defs[p].kind,
                        DefKind::Type | DefKind::Handler
                    )
                }),
                _ => false,
            };
            if !single {
                let found = match (data.kind, data.parent) {
                    (DefKind::Ctor, Some(p)) if self.r.program.defs[p].kind == DefKind::Enum => {
                        "enum variant"
                    }
                    (kind, _) => kind.describe(),
                };
                let name = Some(data.name.clone());
                self.error(range, DiagnosticKind::InvalidUpdateTarget { name, found });
            }
        }
        for arg in args.iter().filter(|a| a.label.is_none()) {
            let range = self.r.program.exprs[arg.value].span.range;
            self.error(range, DiagnosticKind::PositionalInUpdate);
        }
        let fields = args.into_iter().filter(|a| a.label.is_some()).collect();
        ExprKind::Update { ctor, fields, base }
    }

    /// `x.name` is a field or a capability operation; `Module.f` and `Trait.f` are static
    /// references (spec 2.3).
    fn field(&mut self, node: &SyntaxNode) -> ExprKind {
        let name_token = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == LOWER_NAME);
        let base = node.children().find(|c| is_expr(c.kind()));
        let Some(token) = name_token else {
            // `x.` without a name. The syntax diagnostic is already out.
            if let Some(base) = base {
                self.expr(&base);
            }
            return ExprKind::Missing;
        };
        let (name, range) = token_name(&token);
        if let Some(base) = &base
            && base.kind() == PATH_EXPR
            && let Some(path) = base.children().find_map(ast::Path::cast)
        {
            let segments: Vec<SyntaxToken> = path.segments().collect();
            let res = match self.resolve_prefix(&segments) {
                Some(left) => match self.member_of_left(&left, Ns::Value, &name, range) {
                    Some(def) => {
                        self.refer(range, RefTarget::Def(def));
                        Res::Def(def)
                    }
                    None => Res::Err,
                },
                None => Res::Err,
            };
            return ExprKind::Path(res);
        }
        let base = match base {
            Some(b) => self.expr(&b),
            None => self.alloc_expr(ExprKind::Missing, node.text_range()),
        };
        ExprKind::Field {
            base,
            name: Label {
                name,
                span: self.span(range),
            },
        }
    }

    fn binary(&mut self, node: &SyntaxNode) -> ExprKind {
        let lhs = self.child_expr(node, 0);
        let rhs = self.child_expr(node, 1);
        let op = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find_map(|t| {
                Some(match t.kind() {
                    PLUS => BinaryOp::Add,
                    MINUS => BinaryOp::Sub,
                    STAR => BinaryOp::Mul,
                    SLASH => BinaryOp::Div,
                    PERCENT => BinaryOp::Rem,
                    EQ2 => BinaryOp::Eq,
                    NEQ => BinaryOp::Ne,
                    LT => BinaryOp::Lt,
                    LTEQ => BinaryOp::Le,
                    GT => BinaryOp::Gt,
                    GTEQ => BinaryOp::Ge,
                    AMP2 => BinaryOp::And,
                    PIPE2 => BinaryOp::Or,
                    PIPE_GT => BinaryOp::Pipe,
                    _ => return None,
                })
            })
            .unwrap_or(BinaryOp::Add);
        ExprKind::Binary { op, lhs, rhs }
    }

    /// The top-level pattern of an escape arm is an error constructor (spec 7.3).
    fn check_escape_pat(&mut self, pat: PatId) {
        let Pat { kind, span } = &self.r.program.pats[pat];
        let PatKind::Ctor {
            res: Res::Def(def), ..
        } = kind
        else {
            return;
        };
        let data = &self.r.program.defs[*def];
        if data.kind != DefKind::Error {
            let (name, found) = (data.name.clone(), data.kind.describe());
            let range = span.range;
            self.error(
                range,
                DiagnosticKind::WrongKind {
                    name,
                    expected: Expected::Error,
                    found,
                },
            );
        }
    }

    fn pat(&mut self, node: &SyntaxNode, binder: &mut Binder) -> PatId {
        let range = node.text_range();
        let kind = match node.kind() {
            WILDCARD_PAT => PatKind::Wild,
            IDENT_PAT => match child_token(node, LOWER_NAME) {
                Some(token) => {
                    let (name, range) = token_name(&token);
                    PatKind::Bind(self.bind(binder, name, LocalKind::Binding, range))
                }
                None => PatKind::Missing,
            },
            CONST_PAT => match child_token(node, UPPER_NAME) {
                Some(token) => PatKind::Const(self.const_ref(&token)),
                None => PatKind::Missing,
            },
            LITERAL_PAT => pat_literal(node),
            VARIANT_PAT => {
                let res = match node.children().find_map(ast::Path::cast) {
                    Some(path) => self.ctor_path(&path, true),
                    None => Res::Err,
                };
                let (args, rest) = match node.children().find(|c| c.kind() == PAT_ARG_LIST) {
                    Some(list) => {
                        let mut args = Vec::new();
                        let mut rest = false;
                        for arg in list.children() {
                            match arg.kind() {
                                REST_PAT => rest = true,
                                FIELD_PAT => {
                                    let Some(token) = child_token(&arg, LOWER_NAME) else {
                                        continue;
                                    };
                                    let inner = arg.children().find(|c| is_pat(c.kind()));
                                    args.push(self.field_pat(&token, inner.as_ref(), binder));
                                }
                                k if is_pat(k) => {
                                    let pat = self.pat(&arg, binder);
                                    args.push(PatArg { label: None, pat });
                                }
                                _ => {}
                            }
                        }
                        (Some(args), rest)
                    }
                    None => (None, false),
                };
                PatKind::Ctor { res, args, rest }
            }
            UNIT_PAT => PatKind::Unit,
            TUPLE_PAT => PatKind::Tuple(
                node.children()
                    .filter(|c| is_pat(c.kind()))
                    .map(|c| self.pat(&c, binder))
                    .collect(),
            ),
            LIST_PAT => {
                let mut elems = Vec::new();
                let mut rest = None;
                for child in node.children() {
                    if child.kind() == REST_PAT {
                        let binding = child_token(&child, LOWER_NAME).map(|t| {
                            let (name, range) = token_name(&t);
                            self.bind(binder, name, LocalKind::Binding, range)
                        });
                        rest = Some(ListRest {
                            binding,
                            span: self.span(child.text_range()),
                        });
                    } else if is_pat(child.kind()) {
                        elems.push(self.pat(&child, binder));
                    }
                }
                PatKind::List { elems, rest }
            }
            _ => PatKind::Missing,
        };
        self.alloc_pat(kind, range)
    }

    /// `name: pat`, or `name:` (which binds `name`).
    fn field_pat(
        &mut self,
        label: &SyntaxToken,
        inner: Option<&SyntaxNode>,
        binder: &mut Binder,
    ) -> PatArg {
        let (name, range) = token_name(label);
        let pat = match inner {
            Some(p) => self.pat(p, binder),
            None => self.punned_pat(&name, range, binder),
        };
        PatArg {
            label: Some(Label {
                name,
                span: self.span(range),
            }),
            pat,
        }
    }

    fn punned_pat(&mut self, name: &SmolStr, range: TextRange, binder: &mut Binder) -> PatId {
        let local = self.bind(binder, name.clone(), LocalKind::Binding, range);
        self.alloc_pat(PatKind::Bind(local), range)
    }

    fn const_ref(&mut self, token: &SyntaxToken) -> Res {
        let (name, range) = token_name(token);
        let res = self.value(&name, range, UPPER_NAME);
        if let Res::Def(def) = res
            && self.r.program.defs[def].kind != DefKind::Const
        {
            let found = self.r.program.defs[def].kind.describe();
            self.error(
                range,
                DiagnosticKind::WrongKind {
                    name,
                    expected: Expected::Const,
                    found,
                },
            );
            return Res::Err;
        }
        res
    }

    /// The left side of a binding was parsed as an expression, so it is reinterpreted as a
    /// pattern (spec 17.6). Shapes that are not patterns are already reported by the syntax checks.
    fn expr_pat(&mut self, node: &SyntaxNode, binder: &mut Binder) -> PatId {
        let range = node.text_range();
        let kind = match node.kind() {
            UNDERSCORE_EXPR => PatKind::Wild,
            NAME_REF => match node.first_token() {
                Some(t) if t.kind() == LOWER_NAME => {
                    let (name, range) = token_name(&t);
                    PatKind::Bind(self.bind(binder, name, LocalKind::Binding, range))
                }
                Some(t) if t.kind() == UPPER_NAME => PatKind::Const(self.const_ref(&t)),
                _ => PatKind::Missing,
            },
            LITERAL | STRING => {
                let e = self.expr(node);
                match &self.r.program.exprs[e].kind {
                    ExprKind::Lit(lit) => PatKind::Lit(lit.clone()),
                    _ => PatKind::Missing,
                }
            }
            // `-1`: the syntax checks accept only `-` before a number literal.
            PREFIX_EXPR
                if child_token(node, MINUS).is_some()
                    && node.children().count() == 1
                    && node.first_child().is_some_and(|c| {
                        c.kind() == LITERAL
                            && (child_token(&c, INT).is_some() || child_token(&c, FLOAT).is_some())
                    }) =>
            {
                pat_literal(node)
            }
            UNIT_EXPR => PatKind::Unit,
            PAREN_EXPR => {
                return match node.children().find(|c| is_expr(c.kind())) {
                    Some(inner) => self.expr_pat(&inner, binder),
                    None => self.alloc_pat(PatKind::Missing, range),
                };
            }
            PATH_EXPR => match node.children().find_map(ast::Path::cast) {
                Some(path) => PatKind::Ctor {
                    res: self.ctor_path(&path, true),
                    args: None,
                    rest: false,
                },
                None => PatKind::Missing,
            },
            CALL_EXPR => {
                let callee = node.children().find(|c| is_expr(c.kind()));
                let res = match callee
                    .filter(|c| c.kind() == PATH_EXPR)
                    .and_then(|c| c.children().find_map(ast::Path::cast))
                {
                    Some(path) => self.ctor_path(&path, true),
                    None => Res::Err,
                };
                let mut args = Vec::new();
                let mut rest = false;
                if let Some(list) = node.children().find(|c| c.kind() == ARG_LIST) {
                    for arg in list.children() {
                        match arg.kind() {
                            // The `..` of `User(name:, ..)` on the left is parsed as a
                            // SPREAD_ARG without an expression.
                            SPREAD_ARG => rest = true,
                            NAMED_ARG => {
                                let Some(token) = child_token(&arg, LOWER_NAME) else {
                                    continue;
                                };
                                let (name, label_range) = token_name(&token);
                                let pat = match arg.children().find(|c| is_expr(c.kind())) {
                                    Some(inner) => self.expr_pat(&inner, binder),
                                    None => self.punned_pat(&name, label_range, binder),
                                };
                                args.push(PatArg {
                                    label: Some(Label {
                                        name,
                                        span: self.span(label_range),
                                    }),
                                    pat,
                                });
                            }
                            k if is_expr(k) => {
                                let pat = self.expr_pat(&arg, binder);
                                args.push(PatArg { label: None, pat });
                            }
                            _ => {}
                        }
                    }
                }
                PatKind::Ctor {
                    res,
                    args: Some(args),
                    rest,
                }
            }
            TUPLE_EXPR => PatKind::Tuple(
                node.children()
                    .filter(|c| is_expr(c.kind()))
                    .map(|c| self.expr_pat(&c, binder))
                    .collect(),
            ),
            LIST_EXPR => {
                let mut elems = Vec::new();
                let mut rest = None;
                for child in node.children() {
                    if child.kind() == REST_EXPR {
                        let binding = child
                            .children()
                            .find(|c| c.kind() == NAME_REF)
                            .and_then(|c| c.first_token())
                            .filter(|t| t.kind() == LOWER_NAME)
                            .map(|t| {
                                let (name, range) = token_name(&t);
                                self.bind(binder, name, LocalKind::Binding, range)
                            });
                        rest = Some(ListRest {
                            binding,
                            span: self.span(child.text_range()),
                        });
                    } else if is_expr(child.kind()) {
                        elems.push(self.expr_pat(&child, binder));
                    }
                }
                PatKind::List { elems, rest }
            }
            _ => PatKind::Missing,
        };
        self.alloc_pat(kind, range)
    }
}

/// The `X` of `use X` (not an element of a `use` clause).
fn path_in_use_expr(path: &ast::Path) -> bool {
    path.syntax().parent().is_some_and(|p| p.kind() == USE_EXPR)
}

fn pat_literal(node: &SyntaxNode) -> PatKind {
    let negative = child_token(node, MINUS).is_some();
    if let Some(token) = node
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| matches!(t.kind(), INT | FLOAT))
    {
        let mut text = token.text().replace('_', "");
        if negative {
            text.insert(0, '-');
        }
        return PatKind::Lit(if token.kind() == INT {
            Literal::Int(text.into())
        } else {
            Literal::Float(text.into())
        });
    }
    let text: String = node
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == STRING_TEXT)
        .map(|t| string_text(&t))
        .collect();
    PatKind::Lit(Literal::String(text))
}

/// The arguments of an fn's `@external(target, "a", "b")`. Skips annotations whose first argument
/// is not a name (`@external("x")`), and non-string remaining arguments (`@external(js, 1)`,
/// `@external(js, k: "v")`). The parser reports E0138 for both.
fn external_attrs(f: &ast::FnDecl) -> Vec<ExternalAttr> {
    f.annotations()
        .filter(|a| a.name().is_some_and(|n| n.text() == "external"))
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
            })
        })
        .collect()
}

/// The value of a STRING_TEXT token. In a multiline string (spec 2.4), the newline after the
/// opening `"""` and the one before the closing `"""` are dropped, and the closing line's
/// indentation is removed from the start of each line (whitespace-only lines may be shallower;
/// the lexer reports other shallower lines, E0109). This works on the text as written, before
/// unescaping, so that a `\n` escape does not start a line.
fn string_text(token: &SyntaxToken) -> String {
    let Some(string) = token
        .parent()
        .filter(|s| s.first_token().is_some_and(|t| t.kind() == TRIPLE_QUOTE))
    else {
        return unescape(token.text());
    };
    let first = token
        .prev_sibling_or_token()
        .is_some_and(|e| e.kind() == TRIPLE_QUOTE);
    let last = token
        .next_sibling_or_token()
        .is_some_and(|e| e.kind() == TRIPLE_QUOTE);
    // The closing `"""` holds the whitespace before it on its line (`    """`).
    let close = string.last_child_or_token().and_then(|e| e.into_token());
    let indent = close
        .as_ref()
        .filter(|t| t.kind() == TRIPLE_QUOTE)
        .and_then(|t| t.text().strip_suffix("\"\"\""))
        .unwrap_or("");
    let mut text = token.text();
    if first {
        text = text
            .strip_prefix("\r\n")
            .or_else(|| text.strip_prefix('\n'))
            .unwrap_or(text);
    }
    if last {
        text = text
            .strip_suffix("\r\n")
            .or_else(|| text.strip_suffix('\n'))
            .unwrap_or(text);
    }
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        // A token after an interpolation starts in the middle of a line.
        let line = if i == 0 && !first {
            line
        } else if let Some(rest) = line.strip_prefix(indent) {
            rest
        } else if line.trim().is_empty() {
            line.trim_start_matches([' ', '\t'])
        } else {
            line
        };
        out.push_str(line);
    }
    unescape(&out)
}

/// Unescapes `\"` `\\` `\n` `\t` `\r` `\#` `\u{...}`. Invalid escapes are already reported by
/// the lexer, so they are kept as is.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('u') => {
                let rest = chars.as_str();
                let value = rest
                    .strip_prefix('{')
                    .and_then(|r| r.split_once('}'))
                    .and_then(|(digits, _)| u32::from_str_radix(digits, 16).ok())
                    .and_then(char::from_u32);
                match value {
                    Some(v) => {
                        out.push(v);
                        let skip = rest.find('}').unwrap_or(0) + 1;
                        chars = rest[skip..].chars();
                    }
                    None => out.push_str("\\u"),
                }
            }
            Some(c @ ('"' | '\\' | '#')) => out.push(c),
            Some(c) => {
                out.push('\\');
                out.push(c);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn is_expr(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        LITERAL
            | STRING
            | NAME_REF
            | PATH_EXPR
            | UNDERSCORE_EXPR
            | UNIT_EXPR
            | PAREN_EXPR
            | TUPLE_EXPR
            | LIST_EXPR
            | CALL_EXPR
            | FIELD_EXPR
            | PREFIX_EXPR
            | BIN_EXPR
            | USE_EXPR
            | FAIL_EXPR
            | ASSERT_EXPR
            | BLOCK_EXPR
            | IF_EXPR
            | MATCH_EXPR
            | WITH_EXPR
            | ESCAPE_EXPR
            | LAMBDA_EXPR
    )
}

fn is_pat(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        WILDCARD_PAT
            | IDENT_PAT
            | LITERAL_PAT
            | CONST_PAT
            | VARIANT_PAT
            | UNIT_PAT
            | TUPLE_PAT
            | LIST_PAT
    )
}
