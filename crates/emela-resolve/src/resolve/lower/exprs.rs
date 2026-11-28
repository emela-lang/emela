//! Expressions and patterns. They contain each other: a pattern may be written as an expression
//! (`x = Some(y)`), and expressions hold patterns in bindings, arms and parameters.

use emela_syntax::SyntaxKind::{self, *};
use emela_syntax::ast::{self, AstNode};
use emela_syntax::{SyntaxNode, SyntaxToken};
use rowan::TextRange;
use smol_str::SmolStr;

use super::strings::string_text;
use super::{Binder, DeprecatedUse, Lower, UseBind, UseForm, alias_name};
use crate::diagnostic::{DeprecatedRewrite, DiagnosticKind, Expected};
use crate::hir::*;
use crate::resolve::{Ns, child_token, token_name};
use crate::suggest::best_match;

impl Lower<'_, '_> {
    pub(super) fn opt_expr(&mut self, expr: Option<ast::Expr>, range: TextRange) -> ExprId {
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

    pub(super) fn block(&mut self, block: &ast::BlockExpr) -> ExprId {
        self.expr(block.syntax())
    }

    pub(super) fn expr(&mut self, node: &SyntaxNode) -> ExprId {
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
            NAME_REF => {
                let receiver = std::mem::take(&mut self.alias_receiver);
                let res = self.name_ref(node);
                if let Res::Local(local) = res {
                    if let Some(id) = self.alias_use(local, range, receiver) {
                        return id;
                    }
                    self.note_use_bound(local, receiver);
                }
                ExprKind::Path(res)
            }
            PATH_EXPR => match node.children().find_map(ast::Path::cast) {
                Some(path) => {
                    self.member_label = None;
                    let res = self.ctor_path(&path, false);
                    let id = self.alloc_expr(ExprKind::Path(res), range);
                    if let Some(label) = self.member_label.take() {
                        self.r.set_member_label(id, label);
                    }
                    return id;
                }
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
                let id = self.alloc_expr(ExprKind::Use { effect, args }, range);
                let effect = node
                    .text()
                    .to_string()
                    .trim_start_matches("use")
                    .trim()
                    .into();
                self.uses.push(DeprecatedUse {
                    expr: id,
                    range,
                    effect,
                    form: UseForm::Value,
                });
                return id;
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
                let items: Vec<(WithItem, Option<(SmolStr, TextRange)>)> =
                    node.children().filter_map(|c| self.with_item(&c)).collect();
                // A bound name (`src = Files.Open(path)`, `[effect.with]`) is in scope in the
                // body only; the arguments of the items are evaluated outside the `with`.
                self.push_scope();
                let mut binder = Binder::default();
                let handlers = items
                    .into_iter()
                    .map(|(mut item, bind)| {
                        if let Some((name, range)) = bind {
                            item.bind =
                                Some(self.bind(&mut binder, name, LocalKind::Binding, range));
                        }
                        item
                    })
                    .collect();
                self.bind_all(binder);
                let region = self.enter_region();
                let body = self.child_expr(node, 0);
                self.region = region;
                self.pop_scope();
                ExprKind::With { handlers, body }
            }
            LOOP_EXPR => {
                let lp = ast::LoopExpr::cast(node.clone()).expect("LOOP_EXPR");
                return self.loop_expr(&lp);
            }
            LAMBDA_EXPR => {
                self.push_scope();
                let (params, lets) =
                    self.lambda_params(node.children().find_map(ast::ParamList::cast));
                let region = self.enter_region();
                let mut body = self.child_expr(node, 0);
                self.region = region;
                self.pop_scope();
                // `|p| body` is `|t| { p = t; body }` (`[fn.decl]`). A body that is not a block
                // (`|(a, b)| a + b`) is put in one, spanning the body.
                if !lets.is_empty()
                    && let Some(lets) = self.r.prepend_to_block(body, lets)
                {
                    let range = self.r.expr(body).span.range;
                    let block = Block {
                        stmts: lets,
                        tail: Some(body),
                    };
                    body = self.alloc_expr(ExprKind::Block(block), range);
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
            .filter(|c| matches!(c.kind(), BINDING | LOCAL_FN | EFFECT_ALIAS) || is_expr(c.kind()))
            .collect();
        let mut tail = None;
        for (i, child) in children.iter().enumerate() {
            let last = i + 1 == children.len();
            if child.kind() == BINDING {
                stmts.push(self.binding(child));
            } else if let Some(alias) = ast::EffectAlias::cast(child.clone()) {
                self.effect_alias(&alias);
            } else if let Some(f) = ast::LocalFn::cast(child.clone()) {
                stmts.push(self.local_fn(&f));
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

    /// `x <- E[args]` (`[effect.use]`). Binds x for the rest of the block. It adds no statement:
    /// each reference to x becomes its own `use E` ([`Program::alias_uses`]).
    fn effect_alias(&mut self, alias: &ast::EffectAlias) {
        let node = alias.syntax();
        // A right side that is not an effect name is reported by syntax validation (E1113);
        // name resolution does not report it again.
        let effect = match alias.path() {
            Some(path) if !alias.is_malformed() => self.resolve_type_ns(&path, Expected::Effect),
            _ => Res::Err,
        };
        let args = self.type_arg_list(node.children().find(|n| n.kind() == TYPE_ARG_LIST));
        let Some(token) = alias.name() else {
            return;
        };
        let (name, range) = token_name(&token);
        let mut binder = Binder::default();
        let local = self.bind(&mut binder, name, LocalKind::EffectAlias, range);
        self.bind_all(binder);
        self.effect_aliases.insert(local, (effect, args));
    }

    /// A reference to `local`, if it is an effect alias: its own `use E`, recorded in
    /// [`Program::alias_uses`]. Anywhere but the receiver of `x.op(...)` or `x.op` it is E4404;
    /// it is still lowered to `use E`, so that type checking goes on.
    fn alias_use(&mut self, local: LocalId, range: TextRange, receiver: bool) -> Option<ExprId> {
        let (effect, args) = self.effect_aliases.get(&local)?.clone();
        if !receiver {
            let name = self.r.local(local).name.clone();
            self.error(range, DiagnosticKind::EffectAliasAsValue { name });
        }
        let id = self.alloc_expr(ExprKind::Use { effect, args }, range);
        self.r.set_alias_use(id, Some(local));
        Some(id)
    }

    /// Starts a new region ([`Lower::region`]) and returns the one to go back to.
    pub(super) fn enter_region(&mut self) -> u32 {
        self.regions += 1;
        std::mem::replace(&mut self.region, self.regions)
    }

    /// A reference to `local`: if it is bound to `use E`, whether it is still only a receiver in
    /// the region of its binding ([`UseBind`]).
    fn note_use_bound(&mut self, local: LocalId, receiver: bool) {
        let region = self.region;
        if let Some(bind) = self.use_binds.get_mut(&local)
            && (!receiver || bind.region != region)
        {
            bind.receiver_only = false;
        }
    }

    fn set_use_form(&mut self, expr: ExprId, form: UseForm) {
        if let Some(u) = self.uses.iter_mut().rev().find(|u| u.expr == expr) {
            u.form = form;
        }
    }

    /// W4401 for each `use E` written as an expression in the item (`[effect.use]`), with the
    /// replacement in the help: an effect alias where it means the same, and otherwise an effect
    /// alias or a named item.
    pub(super) fn report_deprecated_uses(&mut self) {
        for u in std::mem::take(&mut self.uses) {
            let rewrite = match &u.form {
                UseForm::InPlace { op } => Some(DeprecatedRewrite::InPlace {
                    alias: alias_name(&u.effect),
                    op: op.clone(),
                }),
                UseForm::Binding(local) if self.use_binds[local].receiver_only => {
                    Some(DeprecatedRewrite::Binding {
                        alias: self.r.local(*local).name.clone(),
                    })
                }
                _ => None,
            };
            self.error(
                u.range,
                DiagnosticKind::DeprecatedUse {
                    effect: u.effect,
                    rewrite,
                },
            );
        }
    }

    /// The handler member `member` opened through `local`, if it is an effect alias (E4404).
    fn member_through_alias(&mut self, local: LocalId, range: TextRange, member: &SmolStr) -> bool {
        let Some((effect, _)) = self.effect_aliases.get(&local) else {
            return false;
        };
        let effect = match effect {
            Res::Def(d) => self.r.def(*d).name.clone(),
            _ => SmolStr::new_static("E"),
        };
        let alias = self.r.local(local).name.clone();
        self.error(
            range,
            DiagnosticKind::MemberThroughEffectAlias {
                alias,
                effect,
                member: member.clone(),
            },
        );
        true
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
        if ty.is_none()
            && let PatKind::Bind(local) = self.r.pat(pat).kind
            && binding
                .value()
                .is_some_and(|v| v.syntax().kind() == USE_EXPR)
        {
            self.set_use_form(value, UseForm::Binding(local));
            self.use_binds.insert(
                local,
                UseBind {
                    region: self.region,
                    receiver_only: true,
                },
            );
        }
        Stmt::Let { pat, ty, value }
    }

    /// A local fn (`[fn.local]`). Its name is bound in the block's scope before the body is
    /// resolved, so the body can call it, and stays bound to the end of the block. The parameters
    /// are in a scope of their own, inside it, so a parameter of the same name hides it. Type
    /// names resolve in the scope of the enclosing declaration, its type parameters included.
    fn local_fn(&mut self, f: &ast::LocalFn) -> Stmt {
        let node = f.syntax();
        let (name, range) = match f.name() {
            Some(token) => token_name(&token),
            None => (SmolStr::default(), node.text_range()),
        };
        let local = self.alloc_local(name.clone(), LocalKind::LocalFn, range);
        if !name.is_empty() {
            let mut binder = Binder::default();
            binder.names.push((name, local));
            self.bind_all(binder);
        }
        let ignored = self.ignored_type_params.len();
        let written = node
            .children()
            .filter(|n| n.kind() == TYPE_PARAM_LIST)
            .flat_map(|list| list.descendants_with_tokens())
            .filter_map(|e| e.into_token())
            .filter(|t| t.kind() == UPPER_NAME)
            .map(|t| SmolStr::new(t.text()));
        self.ignored_type_params.extend(written);
        self.push_scope();
        let params = self.params(f.param_list(), LocalKind::Param);
        let sig = self.signature(node, Vec::new(), params, false);
        let region = self.enter_region();
        let body = match f.body() {
            Some(b) => self.block(&b),
            None => self.alloc_expr(ExprKind::Missing, node.text_range()),
        };
        self.region = region;
        self.pop_scope();
        self.ignored_type_params.truncate(ignored);
        Stmt::LocalFn(Box::new(LocalFn {
            local,
            sig,
            body,
            span: self.span(node.text_range()),
        }))
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
                    self.r.set_local_kind(*local, LocalKind::ArmBinding);
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

    pub(super) fn value(&mut self, name: &SmolStr, range: TextRange, kind: SyntaxKind) -> Res {
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
        // `Self` as an expression is the constructor of the enclosing handler. It is not a
        // rename of the handler: the reference lets navigation reach the handler, and renaming
        // leaves the keyword as written.
        if kind == SELF_TYPE_KW {
            return match self.handler {
                Some((_, Some(ctor))) => {
                    self.refer(range, RefTarget::Def(ctor));
                    Res::Def(ctor)
                }
                // A handler without a constructor is already reported.
                Some((_, None)) => Res::Err,
                None => {
                    self.error(range, DiagnosticKind::SelfValueOutside);
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
            let found = self.r.def(def).kind.describe();
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
        let data = &self.r.def(def);
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
            Some(self.r.def(def).kind.describe())
        } else if self.lookup_type_param(name).is_some() {
            Some(DefKind::TypeParam.describe())
        } else if self.r.module_in_scope(self.module, name) {
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
        let (args, spread) = match node.children().find(|c| c.kind() == ARG_LIST) {
            Some(list) => self.arg_list(&list),
            None => (Vec::new(), None),
        };
        match spread {
            Some(base) => {
                // `Self` constructs a handler with its fields only (`[grammar.expr]`). Outside a
                // handler it is already reported (E2108).
                let callee_node = node.children().find(|c| is_expr(c.kind()));
                if self.handler.is_some()
                    && callee_node.is_some_and(|c| {
                        c.kind() == NAME_REF
                            && c.first_token().is_some_and(|t| t.kind() == SELF_TYPE_KW)
                    })
                {
                    let range = self.r.expr(callee).span.range;
                    self.error(range, DiagnosticKind::SelfUpdate);
                }
                self.update(callee, args, base)
            }
            None => ExprKind::Call { callee, args },
        }
    }

    /// The value of the shorthand `name:` of a named argument, short for `name: name`.
    pub(super) fn punned_value(&mut self, name: &SmolStr, range: TextRange) -> ExprId {
        let res = self.value(name, range, LOWER_NAME);
        let value = match res {
            Res::Local(local) => {
                self.note_use_bound(local, false);
                self.alias_use(local, range, false)
            }
            _ => None,
        };
        value.unwrap_or_else(|| self.alloc_expr(ExprKind::Path(res), range))
    }

    /// The arguments in an ARG_LIST, and the expression after a trailing `..`.
    fn arg_list(&mut self, list: &SyntaxNode) -> (Vec<Arg>, Option<ExprId>) {
        let mut args = Vec::new();
        let mut spread = None;
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
                    None => (self.punned_value(&name, range), true),
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
        (args, spread)
    }

    /// An item of `with`, with the name it is bound to (`[effect.with]`).
    fn with_item(&mut self, c: &SyntaxNode) -> Option<(WithItem, Option<(SmolStr, TextRange)>)> {
        if c.kind() == WITH_BIND {
            let name = c
                .children_with_tokens()
                .filter_map(|e| e.into_token())
                .find(|t| t.kind() == LOWER_NAME)
                .map(|t| token_name(&t));
            let inner = c
                .children()
                .find(|n| n.kind() == WITH_BY || ast::Supply::can_cast(n.kind()))?;
            let (item, _) = self.with_item(&inner)?;
            return Some((item, name));
        }
        if c.kind() == WITH_BY {
            let mut supplies = c.children().filter_map(ast::Supply::cast);
            let label = supplies.next()?;
            let res = match label.path() {
                Some(p) => self.resolve_type_ns(&p, Expected::Label),
                None => Res::Err,
            };
            let args = self.supply_args(&label, res);
            let ty_args = self.type_arg_list(label.type_args());
            let mut by_init_args = None;
            let mut by_args = Vec::new();
            let by = supplies.next().and_then(|s| {
                let p = s.path()?;
                let res = self.resolve_type_ns(&p, Expected::HandlerOrEffect);
                by_init_args = self.supply_args(&s, res);
                by_args = self.type_arg_list(s.type_args());
                Some((res, self.span(p.syntax().text_range())))
            });
            return Some((
                WithItem {
                    res,
                    span: self.span(c.text_range()),
                    bind: None,
                    label: None,
                    cap: None,
                    args,
                    ty_args,
                    by: Some(by.unwrap_or((Res::Err, self.span(c.text_range())))),
                    by_args,
                    by_init_args,
                },
                None,
            ));
        }
        let s = ast::Supply::cast(c.clone())?;
        // `fs.Open(path)`: the member is looked up in the effect of the capability by type
        // inference.
        if let Some((cap, member)) = s.cap_member() {
            let (name, range) = token_name(&cap);
            let mut res = self.value(&name, range, LOWER_NAME);
            if let Res::Local(local) = res {
                self.note_use_bound(local, false);
            }
            // A handler member cannot be opened through an effect alias (`[effect.use]`). The
            // item is kept, so that its name is still bound.
            if let Res::Local(local) = res
                && let Some(m) = &member
                && self.member_through_alias(local, range, &token_name(m).0)
            {
                res = Res::Err;
            }
            let cap_expr = self.alloc_expr(ExprKind::Path(res), range);
            let label = member.map(|m| {
                let (name, range) = token_name(&m);
                Label {
                    name,
                    span: self.span(range),
                }
            })?;
            let args = self.supply_args(&s, Res::Err);
            return Some((
                WithItem {
                    res: Res::Err,
                    span: self.span(c.text_range()),
                    bind: None,
                    label: None,
                    cap: Some((cap_expr, label)),
                    args,
                    ty_args: Vec::new(),
                    by: None,
                    by_args: Vec::new(),
                    by_init_args: None,
                },
                None,
            ));
        }
        let p = s.path()?;
        self.member_label = None;
        let res = self.resolve_type_ns(&p, Expected::WithItem);
        let label = self.member_label.take();
        let args = self.supply_args(&s, res);
        let ty_args = self.type_arg_list(s.type_args());
        Some((
            WithItem {
                res,
                span: self.span(p.syntax().text_range()),
                bind: None,
                label,
                cap: None,
                args,
                ty_args,
                by: None,
                by_args: Vec::new(),
                by_init_args: None,
            },
            None,
        ))
    }

    /// The arguments of init written on an item of `with` or `layer` (`WorkerKv(binding: "KV")`,
    /// `[effect.handler]`). `res` is what the item names. They are a call of init, not a
    /// construction of the handler value, so a trailing `..` (a record update, spec 5.3) is
    /// E2111.
    pub(super) fn supply_args(&mut self, supply: &ast::Supply, res: Res) -> Option<SupplyArgs> {
        let list = supply.arg_list()?;
        let (args, spread) = self.arg_list(&list);
        if let Some(base) = spread {
            let range = self.r.expr(base).span.range;
            let (name, found) = match res {
                Res::Def(def) => {
                    let data = &self.r.def(def);
                    (Some(data.name.clone()), data.kind.describe())
                }
                _ => (None, "expression"),
            };
            self.error(range, DiagnosticKind::InvalidUpdateTarget { name, found });
        }
        Some(SupplyArgs {
            args,
            span: self.span(list.text_range()),
        })
    }

    /// A record update `C(f: e, ..base)` (spec 5.3). `C` must have a single constructor (a type,
    /// an error, or a handler with fields), and changed fields are given by name.
    fn update(&mut self, ctor: ExprId, args: Vec<Arg>, base: ExprId) -> ExprKind {
        let Expr { kind, span } = &self.r.expr(ctor);
        let range = span.range;
        let target = match kind {
            ExprKind::Path(Res::Def(def)) => Some(*def),
            // Unresolved names are already reported.
            ExprKind::Path(Res::Err) | ExprKind::Missing => None,
            ExprKind::Path(Res::Local(local)) => {
                let name = self.r.local(*local).name.clone();
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
            let data = &self.r.def(def);
            let single = match data.kind {
                DefKind::Error => true,
                DefKind::Ctor => data.parent.is_some_and(|p| {
                    matches!(self.r.def(p).kind, DefKind::Type | DefKind::Handler)
                }),
                _ => false,
            };
            if !single {
                let found = match (data.kind, data.parent) {
                    (DefKind::Ctor, Some(p)) if self.r.def(p).kind == DefKind::Enum => {
                        "enum variant"
                    }
                    (kind, _) => kind.describe(),
                };
                let name = Some(data.name.clone());
                self.error(range, DiagnosticKind::InvalidUpdateTarget { name, found });
            }
        }
        for arg in args.iter().filter(|a| a.label.is_none()) {
            let range = self.r.expr(arg.value).span.range;
            self.error(range, DiagnosticKind::PositionalInUpdate);
        }
        let fields = args.into_iter().filter(|a| a.label.is_some()).collect();
        ExprKind::Update { ctor, fields, base }
    }

    /// `x.name` is a field or a capability operation; `Module.f` and `Trait.f` are static
    /// references (spec 2.3).
    fn field(&mut self, node: &SyntaxNode) -> ExprKind {
        // A type name after `.` is a handler member of a capability (`fs.Open`, `[effect.use]`).
        let name_token = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| matches!(t.kind(), LOWER_NAME | TYPE_NAME));
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
        let op = token.kind() == LOWER_NAME;
        let base = match base {
            Some(b) => {
                // `x.op` with an effect alias x; `x.M` that opens a handler member is E4404.
                self.alias_receiver = b.kind() == NAME_REF;
                let e = self.expr(&b);
                self.alias_receiver = false;
                if let ExprKind::Use { .. } = self.r.expr(e).kind {
                    match self.r.alias_use(e) {
                        Some(Some(local)) if !op => {
                            let span = self.r.expr(e).span.range;
                            self.member_through_alias(local, span, &name);
                        }
                        // `(use E).op` is `{ h <- E; h.op }`. `(use E).M(...)`, which opens a
                        // handler member, is not expanded (`[effect.use]`).
                        None if op => {
                            self.r.set_alias_use(e, None);
                            self.set_use_form(e, UseForm::InPlace { op: name.clone() });
                        }
                        _ => {}
                    }
                }
                e
            }
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
        let Pat { kind, span } = &self.r.pat(pat);
        let PatKind::Ctor {
            res: Res::Def(def), ..
        } = kind
        else {
            return;
        };
        let data = &self.r.def(*def);
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

    pub(super) fn pat(&mut self, node: &SyntaxNode, binder: &mut Binder) -> PatId {
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
            && self.r.def(def).kind != DefKind::Const
        {
            let found = self.r.def(def).kind.describe();
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
                match &self.r.expr(e).kind {
                    ExprKind::Lit(lit) => PatKind::Lit(lit.clone()),
                    _ => PatKind::Missing,
                }
            }
            // `-1`: the syntax checks accept only `-` before a number literal (E1111).
            PREFIX_EXPR => pat_literal(node),
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
            | LOOP_EXPR
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
