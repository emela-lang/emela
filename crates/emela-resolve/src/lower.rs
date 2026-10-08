//! 宣言の中身を HIR へ写す．局所のスコープ（6.3，6.5，7.3，8.4）と型引数のスコープを持つ．

use emela_syntax::SyntaxKind::{self, *};
use emela_syntax::ast::{self, AstNode, HasAttrs};
use emela_syntax::{SyntaxNode, SyntaxToken};
use rowan::TextRange;
use rustc_hash::FxHashMap;
use smol_str::SmolStr;

use crate::diagnostic::{DiagnosticKind, Expected};
use crate::hir::*;
use crate::resolve::{DotLeft, Ns, Pending, Resolver, child_token, token_name};
use crate::source::ModuleId;
use crate::suggest::best_match;

pub(crate) fn lower_item(resolver: &mut Resolver<'_>, pending: Pending) {
    let mut lower = Lower {
        r: resolver,
        module: pending.module,
        locals: Vec::new(),
        type_params: Vec::new(),
        self_type: false,
    };
    let item = match &pending.item {
        ast::Item::Import(_) => return,
        ast::Item::Fn(f) => Item::Fn(lower.fn_item(pending.def, f)),
        ast::Item::Type(t) => lower.type_item(pending.def, pending.ctor, t),
        ast::Item::Enum(e) => lower.enum_item(pending.def, &pending.children, e),
        ast::Item::Error(e) => {
            let fields = lower.named_fields(e.field_list());
            Item::Error(ErrorItem {
                fields: match fields {
                    Some(fields) => Fields::Named(fields),
                    None => Fields::None,
                },
            })
        }
        ast::Item::Const(c) => {
            let ty = c.ty().map(|t| lower.lower_type(&t));
            let value = lower.opt_expr(c.value(), c.syntax().text_range());
            Item::Const(ConstItem { ty, value })
        }
        ast::Item::Effect(e) => lower.effect_item(&pending.children, e),
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
        ast::Item::Impl(i) => lower.impl_item(i),
    };
    lower.r.program.items.insert(pending.def, item);
}

struct Lower<'r, 'a> {
    r: &'r mut Resolver<'a>,
    module: ModuleId,
    /// 局所変数のスコープ．内側が後ろ．束縛し直すと同じスコープの名前を上書きする．
    locals: Vec<FxHashMap<SmolStr, LocalId>>,
    /// 型引数のスコープ．
    type_params: Vec<FxHashMap<SmolStr, DefId>>,
    /// `Self` を書ける（trait と impl の中）．
    self_type: bool,
}

/// 1つのパターン（か引数の並び）で束縛する名前．同じ名前を2回束縛したら診断を出す．
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

    // -----------------------------------------------------------------------
    // スコープ

    fn push_scope(&mut self) {
        self.locals.push(FxHashMap::default());
    }

    fn pop_scope(&mut self) {
        self.locals.pop();
    }

    /// 束縛した名前を今のスコープに入れる．同じ名前は新しい方が見える（6.3）．
    fn bind_all(&mut self, binder: Binder) {
        if self.locals.is_empty() {
            self.push_scope();
        }
        let scope = self.locals.last_mut().expect("スコープがある");
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

    // -----------------------------------------------------------------------
    // 宣言

    /// `[A: Ord, Item]`．型引数のスコープを1つ積む．呼ぶ側が `pop_type_params` で降ろす．
    fn type_params(&mut self, owner: DefId, list: Option<SyntaxNode>) -> Vec<TypeParam> {
        self.type_params.push(FxHashMap::default());
        let Some(list) = list else {
            return Vec::new();
        };
        let mut params = Vec::new();
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
                self.error(
                    range,
                    DiagnosticKind::Duplicate {
                        name,
                        first: Some(first),
                    },
                );
                continue;
            }
            seen.insert(name.clone(), range);
            // 型名の型引数は外側の同じ名前の型・エフェクト・エラーを隠す（2.3）．
            if token.kind() == TYPE_NAME
                && (self.r.lookup(self.module, Ns::Type, &name).is_some()
                    || self.lookup_type_param(&name).is_some())
            {
                self.error(
                    range,
                    DiagnosticKind::ShadowedByTypeParam { name: name.clone() },
                );
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
                .expect("積んである")
                .insert(name, def);
            params.push((def, param));
        }
        // 制約は全部の型引数を入れてから引く．
        params
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
            .collect()
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
        }
    }

    /// `-> T fails E use R`．`node` は FN_DECL か OP_SIG．
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

    /// 引数の並び．今のスコープに入れる．
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
                // impl と trait の `self`．置けない場所の `self` は構文が診断しないので，ここで見る．
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
        // 名前の読めないバリアントは収集で飛ばしたので，名前のあるものだけを順に対応させる．
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

    fn effect_item(&mut self, ops: &[DefId], e: &ast::EffectDecl) -> Item {
        // 重複した操作は収集で飛ばしたので，名前で対応させる．
        for op in e.ops() {
            let Some(name) = op.name() else { continue };
            let Some(&def) = ops.iter().find(|&&d| {
                let data = &self.r.program.defs[d];
                data.name == name.text() && data.span.map(|s| s.range) == Some(name.text_range())
            }) else {
                continue;
            };
            self.push_scope();
            self.type_params.push(FxHashMap::default());
            let params = self.params(op.param_list(), LocalKind::Param);
            let sig = self.signature(op.syntax(), Vec::new(), params, op.is_suspend());
            self.pop_type_params();
            self.pop_scope();
            self.r.program.items.insert(def, Item::Op(OpItem { sig }));
        }
        Item::Effect(EffectItem { ops: ops.to_vec() })
    }

    fn handler_item(&mut self, def: DefId, ctor: Option<DefId>, h: &ast::HandlerDecl) -> Item {
        let fields = self.named_fields(h.field_list()).unwrap_or_default();
        if let Some(ctor) = ctor {
            self.r.program.items.insert(
                ctor,
                Item::Ctor(CtorItem {
                    owner: def,
                    fields: Fields::Named(fields.clone()),
                }),
            );
        }
        let effect = match h.effect() {
            Some(p) => self.resolve_type_ns(&p, Expected::Effect),
            None => Res::Err,
        };
        // init は with に入るときに走り，ハンドラの値を作る．`self` はまだない．
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
                let op = self.member_of(effect, &name, range);
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
            fields,
            ctor,
            init,
            release,
            ops,
        })
    }

    /// handler の操作と release の暗黙の `self`．位置は handler の名前．
    fn implicit_self(&mut self, handler: DefId) -> LocalId {
        let range = self.r.program.defs[handler]
            .span
            .map_or(TextRange::default(), |s| s.range);
        let local = self.alloc_local("self".into(), LocalKind::SelfParam, range);
        self.locals
            .last_mut()
            .expect("積んである")
            .insert("self".into(), local);
        local
    }

    /// trait の関数か effect の操作を名前で引く．見つからなければ診断を出す．
    fn member_of(&mut self, owner: Res, name: &SmolStr, range: TextRange) -> Res {
        let Res::Def(owner) = owner else {
            return Res::Err;
        };
        if let Some(def) = self.r.member(owner, name) {
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
            let item = self.fn_item(def, &f);
            self.r.program.items.insert(def, Item::TraitFn(item));
        }
        self.self_type = false;
        Item::Trait(TraitItem {
            supertraits,
            fns: fns.to_vec(),
        })
    }

    fn impl_item(&mut self, i: &ast::ImplDecl) -> Item {
        let trait_ref = match i.trait_ref() {
            Some(p) => self.resolve_type_ns(&p, Expected::Trait),
            None => Res::Err,
        };
        self.self_type = true;
        let self_ty = i.self_ty().map(|t| self.lower_type(&t));
        let fns = i
            .fns()
            .filter_map(|f| {
                let token = f.name()?;
                let (name, range) = token_name(&token);
                let trait_fn = self.member_of(trait_ref, &name, range);
                self.push_scope();
                let params = self.params(f.param_list(), LocalKind::Param);
                let body = f.body().map(|b| self.block(&b));
                self.pop_scope();
                Some(ImplFn {
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
        Item::Impl(ImplItem {
            trait_ref,
            self_ty,
            fns,
        })
    }

    // -----------------------------------------------------------------------
    // 型

    fn lower_type(&mut self, node: &SyntaxNode) -> TypeId {
        let range = node.text_range();
        let kind = match node.kind() {
            PATH_TYPE => {
                let res = match node.children().find_map(ast::Path::cast) {
                    Some(path) => self.resolve_type_ns(&path, Expected::Type),
                    None => Res::Err,
                };
                let args = node
                    .children()
                    .find(|n| n.kind() == TYPE_ARG_LIST)
                    .map(|list| list.children().map(|t| self.lower_type(&t)).collect())
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
            UNIT_TYPE => TypeKind::Unit,
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

    /// 型の位置の大文字名．型引数だけを引く．
    fn type_var(&mut self, node: &SyntaxNode) -> Res {
        let Some(token) = child_token(node, UPPER_NAME) else {
            return Res::Err;
        };
        let (name, range) = token_name(&token);
        if let Some(def) = self.lookup_type_param(&name) {
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

    /// `fails` 節．`FAILS_CLAUSE` の中の ERROR_SET．
    fn error_set(&mut self, clause: &SyntaxNode) -> ErrorSet {
        let items = self.set_items(clause, Expected::Error);
        ErrorSet {
            items,
            span: self.span(clause.text_range()),
        }
    }

    fn effect_set(&mut self, clause: &SyntaxNode) -> EffectSet {
        let items = self.set_items(clause, Expected::Effect);
        EffectSet {
            items,
            span: self.span(clause.text_range()),
        }
    }

    fn set_items(&mut self, clause: &SyntaxNode, expected: Expected) -> Vec<(Res, Span)> {
        let Some(set) = clause
            .children()
            .find(|n| matches!(n.kind(), ERROR_SET | EFFECT_SET))
        else {
            return Vec::new();
        };
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

    /// 型の名前空間のパスを引き，種類を確かめる．1区切りなら型引数，モジュールの定義，
    /// Prelude の順．`A.B` ならモジュール `A` の pub な型．
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
            Expected::Type => matches!(
                kind,
                DefKind::Type
                    | DefKind::Enum
                    | DefKind::Error
                    | DefKind::BuiltinType
                    | DefKind::TypeParam
            ),
            Expected::Effect => matches!(kind, DefKind::Effect | DefKind::TypeParam),
            Expected::Error => matches!(kind, DefKind::Error | DefKind::TypeParam),
            Expected::HandlerOrLayer => matches!(kind, DefKind::Handler | DefKind::Layer),
            Expected::Trait => kind == DefKind::Trait,
            _ => true,
        };
        // `use X` には型引数を書けない．
        let fits = fits
            && !(expected == Expected::Effect
                && kind == DefKind::TypeParam
                && path_in_use_expr(path));
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
        Res::Def(def)
    }

    fn undefined_type(&mut self, name: &SmolStr, range: TextRange, expected: Expected) {
        // モジュール名を型の位置に書いた（4.4 の `Email`）．
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

    /// `A.B.f` の `A.B`．先頭は `.` の左として引き，続きはモジュールの中の Trait を引く．
    fn resolve_prefix(&mut self, prefix: &[SyntaxToken]) -> Option<DotLeft> {
        let (first, rest) = prefix.split_first()?;
        let (name, range) = token_name(first);
        let mut left = self.r.lookup_dot_left(self.module, &name, range)?;
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
            left = DotLeft::Trait(def);
        }
        Some(left)
    }

    /// `.` の左の中の名前．モジュールなら名前空間 `ns` の pub な定義，Trait なら関数．
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

    // -----------------------------------------------------------------------
    // 式

    fn opt_expr(&mut self, expr: Option<ast::Expr>, range: TextRange) -> ExprId {
        match expr {
            Some(e) => self.expr(e.syntax()),
            None => self.alloc_expr(ExprKind::Missing, range),
        }
    }

    /// 子の式．なければ `Missing`．
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
                ExprKind::Use { effect }
            }
            FAIL_EXPR => {
                let e = self.child_expr(node, 0);
                self.check_error_ctor(e);
                ExprKind::Fail(e)
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
                    .filter_map(ast::Path::cast)
                    .map(|p| {
                        let res = self.resolve_type_ns(&p, Expected::HandlerOrLayer);
                        (res, self.span(p.syntax().text_range()))
                    })
                    .collect();
                let body = self.child_expr(node, 0);
                ExprKind::With { handlers, body }
            }
            LAMBDA_EXPR => {
                self.push_scope();
                let params = self.params(
                    node.children().find_map(ast::ParamList::cast),
                    LocalKind::LambdaParam,
                );
                let body = self.child_expr(node, 0);
                self.pop_scope();
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

    /// `pat: ty = value`．右辺を先に解決し，後でパターンの名前を入れる（`count = count + 1`）．
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

    /// match と escape の腕．腕ごとにスコープを積み，パターンの名前はガードと本体から見える．
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
                    let text = unescape(t.text());
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

    /// 式の小文字名，大文字名，`self`．
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
                Some(local) => Res::Local(local),
                None => {
                    self.error(range, DiagnosticKind::SelfOutside { self_type: false });
                    Res::Err
                }
            };
        }
        if let Some(local) = self.lookup_local(name) {
            return Res::Local(local);
        }
        if let Some(def) = self.r.lookup(self.module, Ns::Value, name) {
            return Res::Def(def);
        }
        // 大文字名の型引数は型の位置でしか使えない（2.3）．
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

    /// 式とパターンの型名のパス．構成子を引く．`Email.Email` ならモジュールの構成子．
    /// opaque な型の構成子を定義モジュールの外で使えば診断を出す．
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
        Res::Def(def)
    }

    fn undefined_ctor(&mut self, name: &SmolStr, range: TextRange) {
        // 型やエフェクトの名前，モジュール名を値として書いた．
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
        if let Some(list) = node.children().find(|c| c.kind() == ARG_LIST) {
            for arg in list.children() {
                if arg.kind() == NAMED_ARG {
                    let Some(token) = child_token(&arg, LOWER_NAME) else {
                        continue;
                    };
                    let (name, range) = token_name(&token);
                    let label = Label {
                        name: name.clone(),
                        span: self.span(range),
                    };
                    // `NotFound(id:)` は `id: id` の省略．
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
        ExprKind::Call { callee, args }
    }

    /// `x.name` はフィールドか能力の操作，`Module.f` と `Trait.f` は静的な参照（2.3）．
    fn field(&mut self, node: &SyntaxNode) -> ExprKind {
        let name_token = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == LOWER_NAME);
        let base = node.children().find(|c| is_expr(c.kind()));
        let Some(token) = name_token else {
            // `x.` で名前がない．構文の診断が出ている．
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
                    Some(def) => Res::Def(def),
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

    /// `fail e` の `e` が構成子の参照なら，エラーの構成子であることを確かめる（7.2）．
    /// 変数や関数の呼び出しはエラーの値として型推論に任せる．
    fn check_error_ctor(&mut self, e: ExprId) {
        let target = match &self.r.program.exprs[e].kind {
            ExprKind::Path(Res::Def(def)) => Some((*def, e)),
            ExprKind::Call { callee, .. } => match self.r.program.exprs[*callee].kind {
                ExprKind::Path(Res::Def(def)) => Some((def, *callee)),
                _ => None,
            },
            _ => None,
        };
        let Some((def, at)) = target else { return };
        let data = &self.r.program.defs[def];
        if data.kind == DefKind::Ctor {
            let (name, found) = (data.name.clone(), data.kind.describe());
            let range = self.r.program.exprs[at].span.range;
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

    /// escape の腕の最上位のパターンは，エラーの構成子（7.3）．
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

    // -----------------------------------------------------------------------
    // パターン

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

    /// `name: pat`，または `name:`（`name` を束縛する）．
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

    /// 束縛の左辺．式として読んであるので，17.6 のパターンに読み替える．
    /// パターンとして読めない形は構文の検査が診断を出している．
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
                            REST_EXPR => rest = true,
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

/// `use X` の X か（`use` 節の要素ではない）．
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
        .map(|t| unescape(t.text()))
        .collect();
    PatKind::Lit(Literal::String(text))
}

/// `\"` `\\` `\n` `\#` を解く．不正なエスケープは字句が診断を出しているので，そのまま残す．
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
