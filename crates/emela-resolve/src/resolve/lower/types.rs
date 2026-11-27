//! Types, effect and error sets, and the paths of type positions and of the left of `.`.

use emela_syntax::SyntaxKind::*;
use emela_syntax::ast::{self, AstNode};
use emela_syntax::{SyntaxNode, SyntaxToken};
use rowan::TextRange;
use smol_str::SmolStr;

use super::Lower;
use crate::diagnostic::{DiagnosticKind, Expected};
use crate::hir::*;
use crate::resolve::{DotLeft, ModRef, Ns, child_token, token_name};
use crate::suggest::best_match;

impl Lower<'_, '_> {
    pub(super) fn lower_type(&mut self, node: &SyntaxNode) -> TypeId {
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
                    .find(|n| n.kind() == USES_CLAUSE)
                    .map(|c| self.effect_set(&c));
                TypeKind::Fn {
                    params,
                    ret,
                    fails,
                    uses,
                }
            }
            // `use E1 + ... + En` (`[effect.use]`). Each effect item is read as in an effect set.
            CAP_TYPE => {
                let effects = self.effect_set_items(node);
                let vars: Vec<SyntaxNode> =
                    node.children().filter(|n| n.kind() == TYPE_VAR).collect();
                let mut ok = !effects.is_empty();
                for effect in &effects {
                    if let Res::Def(def) = effect.res
                        && self.r.def(def).kind == DefKind::TypeParam
                    {
                        let name = self.r.def(def).name.clone();
                        let range = vars
                            .iter()
                            .find(|n| n.text_range().contains_range(effect.span.range))
                            .map_or(range, |n| n.text_range());
                        self.error(
                            range,
                            DiagnosticKind::WrongKind {
                                name,
                                expected: Expected::Effect,
                                found: DefKind::TypeParam.describe(),
                            },
                        );
                        ok = false;
                    }
                }
                if ok {
                    TypeKind::Cap(effects)
                } else {
                    TypeKind::Missing
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
        if self.ignored_type_params.contains(&name) {
            return Res::Err;
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
    pub(super) fn error_set(&mut self, clause: &SyntaxNode) -> ErrorSet {
        let items = self.set_items(clause, Expected::Error);
        ErrorSet {
            items,
            span: self.span(clause.text_range()),
        }
    }

    pub(super) fn effect_set(&mut self, clause: &SyntaxNode) -> EffectSet {
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
    pub(super) fn type_arg_list(&mut self, list: Option<SyntaxNode>) -> Vec<TypeId> {
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
    ///
    /// In a type argument of a named type an effect also fits [`Expected::Type`]: which arguments
    /// take a set is known only to type inference, which reports E2110 for the others.
    pub(super) fn resolve_type_ns(&mut self, path: &ast::Path, expected: Expected) -> Res {
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
        // A built-in handler with the name of its effect is named where a handler is expected
        // (`[module.resolution]`).
        let def = match expected {
            Expected::WithItem | Expected::HandlerOrLayer | Expected::HandlerOrEffect => {
                self.r.twin_handler(def).unwrap_or(def)
            }
            _ => def,
        };
        let kind = self.r.def(def).kind;
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
            // A handler member in a layer is reported by type inference (E4105).
            Expected::HandlerOrLayer => {
                matches!(kind, DefKind::Handler | DefKind::Layer | DefKind::Member)
            }
            Expected::WithItem => matches!(
                kind,
                DefKind::Handler | DefKind::Layer | DefKind::Effect | DefKind::Member
            ),
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
        if self.r.module_in_scope(self.module, name) {
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
    pub(super) fn resolve_prefix(&mut self, prefix: &[SyntaxToken]) -> Option<DotLeft> {
        let (first, rest) = prefix.split_first()?;
        let (name, range) = token_name(first);
        let mut left = self.r.lookup_dot_left(self.module, &name, range)?;
        match &left {
            DotLeft::Module(ModRef::Source(m)) => self.refer(range, RefTarget::Module(*m)),
            DotLeft::Module(ModRef::Builtin(_)) => {}
            DotLeft::Trait(t) | DotLeft::Effect(t, _) => self.refer(range, RefTarget::Def(*t)),
            DotLeft::ModuleOrEffect(..) => {}
        }
        for segment in rest {
            let (name, range) = token_name(segment);
            let def = self.member_of_left(&left, Ns::Type, &name, range)?;
            if !matches!(self.r.def(def).kind, DefKind::Trait | DefKind::Effect) {
                let found = self.r.def(def).kind.describe();
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
            left = self.r.dot_left_def(def, range);
        }
        Some(left)
    }

    /// A name inside the left of `.`: a pub definition in namespace `ns` for a module, a function
    /// for a trait.
    pub(super) fn member_of_left(
        &mut self,
        left: &DotLeft,
        ns: Ns,
        name: &SmolStr,
        range: TextRange,
    ) -> Option<DefId> {
        match left {
            DotLeft::Module(m) => self.r.module_member(self.module, m, ns, name, range),
            DotLeft::ModuleOrEffect(m, e, effect_range) => {
                let member = (ns != Ns::Value)
                    .then(|| self.r.member(*e, name))
                    .flatten()
                    .filter(|&d| self.r.def(d).kind == DefKind::Member);
                match member {
                    Some(def) => {
                        self.refer(*effect_range, RefTarget::Def(*e));
                        Some(def)
                    }
                    None => {
                        if let ModRef::Source(id) = m {
                            self.refer(*effect_range, RefTarget::Module(*id));
                        }
                        self.r.module_member(self.module, m, ns, name, range)
                    }
                }
            }
            DotLeft::Trait(t) => {
                let found = if ns == Ns::Value {
                    self.r.member(*t, name)
                } else {
                    None
                };
                if found.is_none() {
                    let data = &self.r.def(*t);
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
            // Only handler members are named on an effect (`Files.Open`); its operations are
            // called through a capability (`[effect.use]`).
            DotLeft::Effect(e, effect_range) => {
                if ns == Ns::Value {
                    let name = self.r.def(*e).name.clone();
                    self.error(
                        *effect_range,
                        DiagnosticKind::WrongKind {
                            name,
                            expected: Expected::ModuleOrTrait,
                            found: DefKind::Effect.describe(),
                        },
                    );
                    return None;
                }
                // A label has the handler members of its base (`[effect.label]`).
                let owner = self.r.label_base(*e).unwrap_or(*e);
                let found = self
                    .r
                    .member(owner, name)
                    .filter(|&d| self.r.def(d).kind == DefKind::Member);
                if found.is_some() && owner != *e {
                    self.member_label = Some(*e);
                }
                if found.is_none() {
                    let owner_name = self.r.def(*e).name.clone();
                    let names: Vec<SmolStr> = self
                        .r
                        .members_of(owner)
                        .filter(|&d| self.r.def(d).kind == DefKind::Member)
                        .map(|d| self.r.def(d).name.clone())
                        .collect();
                    let suggestion = best_match(name, &names);
                    self.error(
                        range,
                        DiagnosticKind::UndefinedMember {
                            name: name.clone(),
                            owner: owner_name,
                            owner_kind: "effect",
                            suggestion,
                        },
                    );
                }
                found
            }
        }
    }
}

/// The `X` of `use X` (not an element of a `uses` clause).
/// Whether the path names the effect of `use X` or of the type `use X` (CAP_TYPE, with or without
/// type arguments). Neither takes a type parameter (`[effect.use]`).
fn path_in_use_expr(path: &ast::Path) -> bool {
    let Some(parent) = path.syntax().parent() else {
        return false;
    };
    match parent.kind() {
        USE_EXPR | CAP_TYPE => true,
        PATH_TYPE => parent.parent().is_some_and(|p| p.kind() == CAP_TYPE),
        _ => false,
    }
}
