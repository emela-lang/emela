//! Inference of fn and const bodies: assigns types to expressions and patterns.
//!
//! A [`BodyInfer`] is created per group of mutually referring definitions (a strongly connected
//! component of the call graph), and expressions are inferred bottom-up. On a type mismatch, a
//! diagnostic with the expression's span is pushed and inference continues. Definitions without
//! a declared type (fns without a return type or `uses` clause, consts without a type) are
//! referred to monomorphically within the group and generalized once the group is inferred
//! ([`crate::check`]). Handlers are definitions in the group too: their operation bodies are
//! inferred and their dependencies (the R of the operation bodies) computed.

use crate::sparse::SparseMap;
use emela_resolve::hir::{
    Arg, Arm, BinaryOp, Block, DefId, DefKind, DefModule, ExprId, ExprKind, FnSig, Item, Label,
    Literal, LocalId, Param, PatId, PatKind, Program, Res, Span, Stmt, StrPart, TypeId, UnaryOp,
    int_literal_value,
};
use la_arena::{ArenaMap, Idx};
use rustc_hash::{FxHashMap, FxHashSet};
use smol_str::SmolStr;

use crate::builtin::Builtins;
use crate::coverage::Shapes;
use crate::diagnostic::{Diagnostic, DiagnosticKind, EffectContext};
use crate::exhaustiveness::{Witness, check_match};
use crate::infer::{InferCtx, Origin, SetKind, substitute};
use crate::lower::{
    Omitted, Scope, TyTable, effect_params, has_omitted_fails, is_intrinsic_effect,
};
use crate::row::{CapElem, Row, RowElem, RowVar, Tag};
use crate::signature::{FnParams, Signature, Signatures, param_labels};
use crate::traits::Traits;
use crate::ty::{ParamKind, Prim, Ty, TyParam, TyVar};

mod args;
mod bound;
mod calls;
mod capabilities;
mod effects;
mod errors;
mod handlers;
mod items;
mod lambdas;
mod members;
mod methods;
mod patterns;
mod refs;
mod suspend;
mod with;

/// The result of inferring one group of definitions. Types have unification bindings followed.
pub(crate) struct BodyTypes {
    pub(crate) exprs: SparseMap<ExprId, Ty>,
    pub(crate) pats: SparseMap<PatId, Ty>,
    pub(crate) locals: SparseMap<LocalId, Ty>,
    /// The definition whose body a local variable belongs to.
    pub(crate) local_owner: SparseMap<LocalId, DefId>,
    pub(crate) fields: SparseMap<ExprId, usize>,
    pub(crate) calls: SparseMap<ExprId, Vec<Option<ExprId>>>,
    pub(crate) type_args: SparseMap<ExprId, Vec<Ty>>,
    /// References to generic definitions → the sets instantiating set type parameters.
    pub(crate) type_rows: SparseMap<ExprId, Vec<Row>>,
    /// For definitions without a declared type: generalized set variables and the indices of
    /// their type parameters.
    pub(crate) generalized_rows: Vec<(DefId, Vec<(RowVar, u32)>)>,
    /// For definitions without a declared type: generalized type variables and the indices of
    /// their type parameters.
    pub(crate) generalized_tys: Vec<(DefId, Vec<(TyVar, u32)>)>,
    pub(crate) pat_fields: SparseMap<PatId, Vec<Option<PatId>>>,
    pub(crate) diags: Vec<Diagnostic>,
    /// The generalized declared types of definitions that had none.
    pub(crate) inferred: Vec<(DefId, Signature)>,
    /// The error set E of each expression (spec 9.4).
    pub(crate) errors: SparseMap<ExprId, Row>,
    /// The effect set R of each expression (spec 9.4).
    pub(crate) effects: SparseMap<ExprId, Row>,
    /// References to capability operations (the `Field` expression of `log.info`) → the effect
    /// operation's definition.
    pub(crate) ops: SparseMap<ExprId, DefId>,
    /// Openings of handler members (`[effect.handler]`, `[effect.with]`) → the member: the callee
    /// of `Files.Open(p)` or `fs.Open(p)` in an init, and the capability of an item `x = fs.Open(p)`.
    pub(crate) members: SparseMap<ExprId, DefId>,
    /// Handler members opened inside an init through a handler that a `with` inside the init
    /// provides → that `with`: they are released at its end (`[effect.handler]`).
    pub(crate) member_open_withs: SparseMap<ExprId, ExprId>,
    /// The `use E` expressions (W4401) and the effect aliases written as a value (E4404) for
    /// which a bound item would send the other uses of E by name in the body of the `with` that
    /// provides E to an outer handler (`[effect.with]`): their help does not suggest one.
    pub(crate) bound_item_redirects: Vec<Span>,
    /// Method calls (the `Field` callee of `x.f(a)`) → the function they call.
    pub(crate) methods: SparseMap<ExprId, DefId>,
    /// Dependencies of the group's handlers (the union of the R of init, release and the
    /// operation bodies).
    pub(crate) handler_uses: Vec<(DefId, Row)>,
    /// The init E of the group's handlers.
    pub(crate) handler_fails: Vec<(DefId, Row)>,
    /// `with` expressions → handlers in initialization order (layers expanded, later handlers
    /// overriding earlier ones).
    pub(crate) with_handlers: SparseMap<ExprId, crate::layer::WithInfo>,
    /// `with` expressions → per item, the arguments of init reordered by parameter.
    pub(crate) with_args: SparseMap<ExprId, Vec<Vec<Option<ExprId>>>>,
    /// Arguments whose capability is passed as fewer effects (`[effect.use]`) → the effects of
    /// the parameter.
    pub(crate) narrowings: SparseMap<ExprId, Vec<Tag>>,
    /// `self` passed as the capability of its handler ([`BodyInfer::self_as_capability`]).
    pub(crate) self_caps: FxHashSet<ExprId>,
    /// Layers → per member, the arguments of init reordered by parameter.
    pub(crate) layer_args: Vec<(DefId, Vec<Vec<Option<ExprId>>>)>,
    /// `with` expressions → per item, the type arguments of the handler with type parameters
    /// that supplies it (`[effect.with]`). Empty for other items.
    pub(crate) with_ty_args: SparseMap<ExprId, Vec<Vec<Ty>>>,
    /// Layers → per member, the type arguments of a handler with type parameters
    /// (`[effect.layer]`).
    pub(crate) layer_ty_args: Vec<(DefId, Vec<Vec<Ty>>)>,
}

/// A handler operation body ([`BodyInfer::infer_handler`]), used when computing E and R.
struct HandlerBody {
    handler: DefId,
    /// The operation's index within the handler (`HandlerItem::ops`).
    index: usize,
    body: ExprId,
    /// The E of the effect operation's declaration.
    fails: Row,
    /// A suspend fn implementation that uses `resume` other than by calling it directly (spec
    /// 8.7). Async is added to the dependencies.
    resume_later: bool,
    /// Whether it implements a suspend fn. fn operation bodies cannot suspend (E4303).
    suspend: bool,
    /// The `resume` parameter of a suspend fn implementation, for E4305.
    resume: Option<LocalId>,
}

/// A handler's init or release body, used when computing E and R.
struct HandlerPart {
    handler: DefId,
    body: ExprId,
    /// Otherwise init. release's E must be empty (spec 8.4).
    release: bool,
}

/// A local fn of a body being inferred (`[fn.local]`), for computing E and R once the types are
/// settled.
struct LocalFnInfo {
    /// The parameter names, the labels of its named arguments (`[fn.args]`).
    labels: Vec<Option<SmolStr>>,
    body: ExprId,
    /// The E and R of its type. The E and R of the body are contained in them.
    fails: Row,
    uses: Row,
    /// Whether its E is declared (`-> T` is written, with or without `fails`). Then the E of the
    /// body is checked against it (E3201); otherwise it is a set variable.
    fails_declared: bool,
    /// Whether `fails` is written, for the message of E3201.
    fails_written: bool,
    /// Whether `uses` is written. Then the R of the body is checked against it (E4301).
    uses_written: bool,
}

/// Handler facts computed in earlier groups.
pub(crate) struct HandlerFacts<'a> {
    /// Dependencies (the union of the R of init, release and the operation bodies).
    pub(crate) uses: &'a ArenaMap<DefId, Row>,
    /// init's E, which goes into the E of `with` (spec 9.4).
    pub(crate) fails: &'a ArenaMap<DefId, Row>,
    /// The type arguments of the members of layers (`[effect.layer]`).
    pub(crate) layer_ty_args: &'a ArenaMap<DefId, Vec<Vec<Ty>>>,
}

/// State while inferring one group of definitions.
pub(crate) struct BodyInfer<'a> {
    program: &'a Program,
    ty_table: &'a TyTable,
    sigs: &'a Signatures,
    /// Handler dependencies and init E computed in earlier groups.
    handler_facts: HandlerFacts<'a>,
    /// Definitions whose R must be empty (`main` and test functions).
    closed: &'a FxHashMap<DefId, EffectContext>,
    /// Built-in effects and scope functions (spec chapter 11).
    builtins: &'a Builtins,
    current: Option<DefId>,
    /// The module of the definition being inferred, for deciding whether opaque fields are
    /// accessible.
    module: DefModule,
    /// Type parameters of the fn being inferred.
    type_params: Vec<DefId>,
    /// Kinds of the fn's type parameters, in the same order as `type_params`.
    param_kinds: Vec<ParamKind>,
    /// What `Self` stands for in the definition being inferred: the impl's type in an impl's
    /// function, `Bound(0)` in a trait's default implementation.
    self_ty: Option<Ty>,
    /// Supertraits and impls.
    traits: &'a Traits,
    ctx: InferCtx,
    /// Monomorphic types and type parameters (written and invisible set ones) of the group's
    /// definitions without declared types. Generalized once the group is inferred.
    pending: Vec<(DefId, Ty, Vec<TyParam>)>,
    /// Inferred definitions (fns with bodies and consts), in inference order.
    defs: Vec<DefId>,
    /// Lambdas checked against a function type that quantifies capability sets (`[type.rules]`):
    /// their unknown sets ([`CapId::Rigid`]) are removed from the R of the body, since the R of
    /// such a type has them.
    quantified_lambdas: FxHashMap<ExprId, Vec<CapElem>>,
    /// Lambda expressions → the set variables of their type's error and effect sets, which
    /// include the body's E and R (`body/errors.rs`, `body/effects.rs`).
    lambda_rows: SparseMap<ExprId, (RowVar, RowVar)>,
    /// Local fns (`[fn.local]`), by the local that names them.
    local_fns: SparseMap<LocalId, LocalFnInfo>,
    /// Set variables for the dependencies and the init E of the group's handlers.
    pending_handlers: Vec<(DefId, RowVar, RowVar)>,
    handler_bodies: Vec<HandlerBody>,
    handler_parts: Vec<HandlerPart>,
    /// The handler whose init is being inferred. A handler value's constructor can only be used
    /// in its own init (E4005).
    in_init: Option<DefId>,
    /// The value each local variable bound by `x = value` was bound to: a capability in a field
    /// of a handler stays one through such a binding (`p = self.pool`,
    /// [`Self::field_capability`]).
    let_values: FxHashMap<LocalId, ExprId>,
    /// References from other definitions in the group to `pending` definitions with written type
    /// parameters. Checked against the definitions' final types once the group is inferred
    /// ([`Self::finish`]).
    ///
    /// Such a reference replaces the written type parameters, and the parts not yet known, with
    /// fresh type variables. Unifying with the definition's type directly would leak its type
    /// parameters into the other definitions. An inferred E or R of the referenced fn goes
    /// through `instance_sets` instead.
    deferred: Vec<DeferredRef>,
    /// The inferred E and R of the definitions that `deferred` references refer to, placed at
    /// each reference once solved (`body/errors.rs`).
    instance_sets: Vec<InstanceSet>,
    locals: SparseMap<LocalId, Ty>,
    local_owner: SparseMap<LocalId, DefId>,
    /// The anonymous function or local fn whose body is being inferred: the lambda expression, or
    /// the body of the local fn. `None` in the body of the definition itself.
    binder: Option<ExprId>,
    /// The binder each local variable is bound in ([`Self::binder`]). A local used inside another
    /// binder is captured (`[effect.use]`).
    local_binder: SparseMap<LocalId, Option<ExprId>>,
    /// While computing R: the `with` expressions enclosing the expression, innermost last, with
    /// the effects they provide (`body/effects.rs`).
    with_stack: Vec<(ExprId, Vec<Tag>)>,
    /// While computing R: the callees of calls and pipes, so that an operation of an effect alias
    /// that is called is told from one taken as a function value (`body/effects.rs`).
    called: rustc_hash::FxHashSet<ExprId>,
    /// Field access expressions → field indices.
    fields: SparseMap<ExprId, usize>,
    /// Call, pipe and update expressions → the arguments reordered by position.
    calls: SparseMap<ExprId, Vec<Option<ExprId>>>,
    /// References to generic definitions → instantiated type arguments.
    type_args: SparseMap<ExprId, Vec<Ty>>,
    /// References to generic definitions → the sets instantiating set type parameters.
    type_rows: SparseMap<ExprId, Vec<Row>>,
    /// Constructor patterns → the patterns reordered by field position.
    pat_fields: SparseMap<PatId, Vec<Option<PatId>>>,
    exprs: SparseMap<ExprId, Ty>,
    pats: SparseMap<PatId, Ty>,
    diags: Vec<Diagnostic>,
    /// Numeric type constraints, checked when the body is done ([`Self::check_numbers`]).
    numbers: Vec<NumConstraint>,
    /// Matches and bindings to check for coverage once types are settled
    /// ([`Self::check_coverage`]).
    coverage: Vec<CoverageSite>,
    /// Each expression's error set, computed after body inference (`body/errors.rs`).
    expr_rows: SparseMap<ExprId, Row>,
    /// Set variables that remove elements from a set variable's solution (`δ ⊇ v ∖ elems`): the
    /// tags caught by `escape` or provided by `with`, and the capabilities whose root or binding
    /// ends (`[effect.use]`). A tag removes only the same tag, not a capability of that effect.
    row_diffs: Vec<(RowVar, RowVar, Vec<RowElem>)>,
    /// Where each capability first entered each set variable, for E4405 (`[effect.escape]`).
    cap_provenance: FxHashMap<(RowVar, crate::row::CapId), Option<Span>>,
    /// `escape` inner expressions, the errors their arms catch and their arm `_`, for the W3201
    /// check.
    escapes: Vec<errors::Escape>,
    /// The call on the right side of a pipe `x |> f(a)` → the pipe expression, where its
    /// arguments are recorded (`body/effects.rs`).
    piped_calls: SparseMap<ExprId, ExprId>,
    /// Each expression's effect set, computed after body inference (`body/effects.rs`).
    expr_effects: SparseMap<ExprId, Row>,
    /// References to capability operations → effect operations.
    ops: SparseMap<ExprId, DefId>,
    /// Openings of handler members → the member ([`BodyTypes::members`]).
    member_opens: SparseMap<ExprId, DefId>,
    /// See [`BodyResult::member_open_withs`].
    member_open_withs: SparseMap<ExprId, ExprId>,
    /// See [`BodyTypes::bound_item_redirects`].
    bound_item_redirects: Vec<Span>,
    /// The `with`s inside the init being inferred, outermost first.
    init_withs: Vec<ExprId>,
    /// See [`BodyResult::narrowings`].
    narrowings: SparseMap<ExprId, Vec<Tag>>,
    /// See [`BodyResult::self_caps`].
    self_caps: FxHashSet<ExprId>,
    /// Callees of calls, where a handler member may be opened inside an init.
    member_callees: FxHashSet<ExprId>,
    /// Variables bound to the capability a handler member item of `with` opens. Its operations
    /// add the variable to R, which the `with` removes (`[effect.with]`).
    member_bound: FxHashSet<LocalId>,
    /// While inferring types: the `with` expressions around the expression, innermost last, with
    /// the effects they provide by name. `None` marks the body of an anonymous function or a local
    /// fn, whose R the withs around it do not provide (`[effect.with]`).
    name_providers: Vec<Option<(ExprId, Vec<Tag>)>>,
    /// The R of the function being called, for the arguments checked next ([`Self::check_args`]).
    callee_uses: Option<Row>,
    /// The effects that the function being called provides by name to the lambda checked next
    /// (`[effect.escape]`).
    provided_to_lambda: Option<Vec<Tag>>,
    /// `with` expressions → the items that open handler members, with the types of opening them.
    with_members: SparseMap<ExprId, Vec<(usize, crate::ty::FnTy)>>,
    /// Method calls (the `Field` callee) → the fn or trait function called (`[type.rules]`).
    methods: SparseMap<ExprId, DefId>,
    /// `with` expressions with their listed handlers (layers expanded) and dependencies. The
    /// initialization order and dependency cycles are decided after sets are solved
    /// (`body/effects.rs`).
    withs: Vec<crate::layer::PendingWith>,
    /// `with` expressions → providers in initialization order.
    with_handlers: SparseMap<ExprId, crate::layer::WithInfo>,
    /// `with` expressions → per item, the arguments of init reordered by parameter
    /// ([`Self::infer_supply_args`]). Empty for an item without them.
    with_args: SparseMap<ExprId, Vec<Vec<Option<ExprId>>>>,
    /// Layers → per member, the arguments of init reordered by parameter.
    layer_args: Vec<(DefId, Vec<Vec<Option<ExprId>>>)>,
    /// `with` expressions → per item, the type arguments it gives a handler with type
    /// parameters (`body/items.rs`).
    item_insts: SparseMap<ExprId, Vec<items::ItemInst>>,
    /// The set variables that are the set type arguments of a built-in handler at an item
    /// (`Group[E, R]`), with the `with` and the index of the item. Such a set may have the
    /// capabilities of the items before it in the same `with` (`[task.group]`).
    item_set_vars: rustc_hash::FxHashMap<crate::row::RowVar, (ExprId, u32)>,
    /// Layers of this group → per member, the type arguments it gives a handler with type
    /// parameters.
    layer_insts: Vec<(DefId, Vec<items::ItemInst>)>,
    /// Effect sets required to be `Immediate` (`body/suspend.rs`).
    immediates: Vec<ImmediateObligation>,
    /// Types required to satisfy a trait by the constraints of instantiated type parameters,
    /// checked once the group's types are settled ([`Self::check_traits`]).
    trait_obligations: Vec<TraitObligation>,
    /// The trait constraints of the type parameters of handlers instantiated at items
    /// (`body/items.rs`), checked once the effects of the group are inferred.
    item_obligations: Vec<TraitObligation>,
    /// The `Self` of referenced trait functions that only the result type determines, checked
    /// to be known once the group's types are settled ([`Self::check_known_selves`]).
    self_obligations: Vec<SelfObligation>,
    /// Set variables that get an `Immediate` bound when generalized.
    immediate_vars: rustc_hash::FxHashSet<RowVar>,
}

/// The parameters and body of a function definition: a top-level fn, a trait's function or an
/// impl's function.
pub(crate) struct FnParts<'p> {
    pub(crate) params: &'p [Param],
    pub(crate) body: Option<ExprId>,
    /// The declaration the type comes from: the trait function's for an impl function. `None`
    /// for the Prelude's traits, which have no source.
    pub(crate) sig: Option<&'p FnSig>,
}

pub(crate) fn fn_parts(program: &Program, def: DefId) -> Option<FnParts<'_>> {
    match program.item(def) {
        Some(Item::Fn(item) | Item::TraitFn(item)) => Some(FnParts {
            params: &item.sig.params,
            body: item.body,
            sig: Some(&item.sig),
        }),
        Some(_) => None,
        None => {
            let (_, f) = program.impl_fn(def)?;
            let sig = match f.trait_fn {
                Res::Def(t) => match program.item(t) {
                    Some(Item::TraitFn(item)) => Some(&item.sig),
                    _ => None,
                },
                _ => None,
            };
            Some(FnParts {
                params: &f.params,
                body: f.body,
                sig,
            })
        }
    }
}

impl<'a> BodyInfer<'a> {
    pub(crate) fn new(
        program: &'a Program,
        ty_table: &'a TyTable,
        sigs: &'a Signatures,
        handler_facts: HandlerFacts<'a>,
        closed: &'a FxHashMap<DefId, EffectContext>,
        builtins: &'a Builtins,
        traits: &'a Traits,
    ) -> Self {
        let mut ctx = InferCtx::new();
        ctx.set_error_tags(ty_table.error_tags());
        BodyInfer {
            program,
            ty_table,
            sigs,
            handler_facts,
            closed,
            builtins,
            current: None,
            module: DefModule::Prelude,
            type_params: Vec::new(),
            param_kinds: Vec::new(),
            self_ty: None,
            traits,
            ctx,
            pending: Vec::new(),
            defs: Vec::new(),
            lambda_rows: SparseMap::new(),
            quantified_lambdas: FxHashMap::default(),
            local_fns: SparseMap::new(),
            pending_handlers: Vec::new(),
            handler_bodies: Vec::new(),
            handler_parts: Vec::new(),
            in_init: None,
            let_values: FxHashMap::default(),
            deferred: Vec::new(),
            instance_sets: Vec::new(),
            locals: SparseMap::new(),
            local_owner: SparseMap::new(),
            fields: SparseMap::new(),
            calls: SparseMap::new(),
            type_args: SparseMap::new(),
            type_rows: SparseMap::new(),
            pat_fields: SparseMap::new(),
            exprs: SparseMap::new(),
            pats: SparseMap::new(),
            diags: Vec::new(),
            numbers: Vec::new(),
            coverage: Vec::new(),
            expr_rows: SparseMap::new(),
            row_diffs: Vec::new(),
            called: Default::default(),
            piped_calls: SparseMap::default(),
            binder: None,
            local_binder: SparseMap::default(),
            with_stack: Vec::new(),
            escapes: Vec::new(),
            cap_provenance: FxHashMap::default(),
            expr_effects: SparseMap::new(),
            ops: SparseMap::new(),
            member_opens: SparseMap::new(),
            member_open_withs: SparseMap::new(),
            bound_item_redirects: Vec::new(),
            init_withs: Vec::new(),
            narrowings: SparseMap::new(),
            self_caps: FxHashSet::default(),
            member_callees: FxHashSet::default(),
            member_bound: FxHashSet::default(),
            name_providers: Vec::new(),
            callee_uses: None,
            provided_to_lambda: None,
            with_members: SparseMap::new(),
            methods: SparseMap::new(),
            withs: Vec::new(),
            with_handlers: SparseMap::new(),
            with_args: SparseMap::new(),
            layer_args: Vec::new(),
            item_insts: SparseMap::new(),
            item_set_vars: Default::default(),
            layer_insts: Vec::new(),
            immediates: Vec::new(),
            trait_obligations: Vec::new(),
            item_obligations: Vec::new(),
            self_obligations: Vec::new(),
            immediate_vars: Default::default(),
        }
    }

    /// Switches to the definition about to be inferred, taking its module and type parameters.
    fn enter(&mut self, def: DefId) {
        self.current = Some(def);
        self.ctx.scopes.reset();
        self.binder = None;
        self.module = self.program.defs[def].module.clone();
        self.self_ty = None;
        (self.type_params, self.param_kinds) = match self.program.item(def) {
            // The handler's type parameters (`[effect.handler]`), or for an inner handler those
            // of its outer handler.
            Some(Item::Handler(_)) => {
                let defs = crate::lower::type_param_defs(self.program, def);
                let kinds = vec![ParamKind::Type; defs.len()];
                (defs, kinds)
            }
            Some(Item::Fn(item)) => {
                let params = FnParams::new(self.program, self.ty_table, item);
                (params.defs, params.kinds)
            }
            Some(Item::TraitFn(item)) => {
                let trait_def = self.program.defs[def].parent;
                let params = FnParams::with_self(self.program, self.ty_table, item, trait_def);
                self.self_ty = params.self_ty.clone();
                (params.defs, params.kinds)
            }
            // The impl's type parameters can be named in its functions; the trait function's own
            // cannot.
            _ => match self.program.impl_fn(def) {
                Some((impl_def, _)) => {
                    let defs: Vec<DefId> = match self.program.item(impl_def) {
                        Some(Item::Impl(i)) => i.type_params.iter().map(|tp| tp.def).collect(),
                        _ => Vec::new(),
                    };
                    self.self_ty = self.traits.head(impl_def).map(|h| h.self_ty.clone());
                    let kinds = vec![ParamKind::Type; defs.len()];
                    (defs, kinds)
                }
                None => (Vec::new(), Vec::new()),
            },
        };
    }

    /// Gives a definition without a declared type the monomorphic type used within the group: for
    /// a fn, the written parameter types, the return type (a type variable if not written), and E
    /// and R (set variables if not written); for a const, a type variable; for a handler, set
    /// variables for its dependencies and init E.
    pub(crate) fn declare(&mut self, def: DefId) {
        self.enter(def);
        let program = self.program;
        // A layer has no type; only the arguments of its members are inferred.
        if let Some(Item::Layer(_)) = program.item(def) {
            return;
        }
        if let Some(Item::Handler(_)) = program.item(def) {
            let uses = self.ctx.new_row_var();
            let fails = self.ctx.new_row_var();
            self.pending_handlers.push((def, uses, fails));
            return;
        }
        let (ty, params) = match program.item(def) {
            Some(Item::Fn(item)) => {
                let mut fn_params = FnParams::new(program, self.ty_table, item);
                let params = fn_params.lower_params(
                    program,
                    self.ty_table,
                    &item.sig.params,
                    &mut self.diags,
                );
                let (ret, fails) = match item.sig.ret {
                    Some(ret_ref) => {
                        let mut scope = fn_params.result_scope(Omitted::Fresh(&mut self.ctx));
                        let ret = self.ty_table.lower_in(
                            program,
                            &mut scope,
                            ret_ref,
                            true,
                            &mut self.diags,
                        );
                        let declared = fn_params.declared_fails(
                            program,
                            self.ty_table,
                            &item.sig,
                            &mut self.diags,
                        );
                        if has_omitted_fails(program, ret_ref) {
                            // E is inferred from the body and checked to be contained in the
                            // declared E (the written fails clause and invisible sets). If the
                            // returned function only calls a parameter function, this fn itself
                            // does not fail (compose in spec 9.2).
                            let fails = Row::var(self.ctx.new_row_var());
                            self.ctx
                                .add_subset(fails.clone(), declared, Origin::Body(def));
                            (ret, fails)
                        } else {
                            // Return type and E as declared; only R is inferred.
                            (ret, declared)
                        }
                    }
                    None => (self.ctx.new_var(), Row::var(self.ctx.new_row_var())),
                };
                // Without a `uses` clause, R is inferred from the body (spec 9.3).
                let uses = match item.sig.uses {
                    Some(_) => {
                        fn_params.declared_uses(program, self.ty_table, &item.sig, &mut self.diags)
                    }
                    None => Row::var(self.ctx.new_row_var()),
                };
                (
                    Ty::func_sets(params, ret, fails, uses),
                    fn_params.all_params(),
                )
            }
            _ => (self.ctx.new_var(), Vec::new()),
        };
        self.pending.push((def, ty, params));
    }

    /// Places the body at the declared type (or the type given by [`Self::declare`]).
    pub(crate) fn infer_def(&mut self, def: DefId) {
        self.enter(def);
        self.defs.push(def);
        let program = self.program;
        if let Some(Item::Handler(item)) = program.item(def) {
            self.infer_handler(def, item);
            return;
        }
        if let Some(Item::Layer(item)) = program.item(def) {
            let mut insts = Vec::new();
            let args = item
                .members
                .iter()
                .map(|m| {
                    let mut inst = self.instantiate_item(m.res, &m.ty_args, m.span, true);
                    let args = self.infer_supply_args(m.res, m.args.as_ref(), m.span, &inst.args);
                    self.freeze(&mut inst);
                    insts.push(inst);
                    args
                })
                .collect();
            self.layer_args.push((def, args));
            self.layer_insts.push((def, insts));
            return;
        }
        let declared = match self.sigs.get(def) {
            Some(sig) => sig.scheme.ty.clone(),
            None => match self.pending_ty(def) {
                Some((ty, _)) => ty,
                None => return,
            },
        };
        match program.item(def) {
            Some(Item::Const(item)) => self.check_expr(item.value, &declared),
            _ => {
                if let Some(parts) = fn_parts(program, def) {
                    let in_impl = program.defs[def].kind == DefKind::ImplFn;
                    self.infer_fn(parts.params, parts.body, &declared, in_impl);
                }
            }
        }
    }

    /// `declared` is the fn's type. An impl's function (`in_impl`) may write parameter types; they
    /// must be the trait's (spec 10.2). A parameter beyond the declared ones has a broken type
    /// (E3101 is reported with the impl).
    fn infer_fn(&mut self, params: &[Param], body: Option<ExprId>, declared: &Ty, in_impl: bool) {
        let Ty::Fn(f) = declared else {
            return;
        };
        for (i, p) in params.iter().enumerate() {
            let ty = f.params.get(i).cloned().unwrap_or(Ty::Error);
            if in_impl && let Some(written) = p.ty {
                let written_ty = self.lower_type(written);
                let span = self.program.types[written]
                    .span
                    .unwrap_or(self.program.locals[p.local].span);
                self.coerce_at(span, &ty, &written_ty);
            }
            self.set_local(p.local, ty);
        }
        if let Some(body) = body {
            self.check_expr(body, &f.ret);
        }
    }

    /// For an inner handler, the E its init may have: the `fails` of its handler member.
    pub(super) fn inner_init_fails(&self, handler: DefId) -> Option<Row> {
        let Some(Item::Handler(item)) = self.program.item(handler) else {
            return None;
        };
        let Some(Res::Def(member)) = item.member else {
            return None;
        };
        self.inner_member_ty(handler, member).map(|f| f.fails)
    }

    /// A handler's init E, which goes into the E of `with` (spec 9.4).
    fn handler_init_fails(&self, handler: DefId) -> Row {
        if let Some(row) = self.handler_facts.fails.get(handler) {
            return row.clone();
        }
        match self.pending_handlers.iter().find(|(h, _, _)| *h == handler) {
            Some(&(_, _, v)) => Row::var(v),
            None => Row::empty(),
        }
    }

    /// Whether the definition being inferred is a handler.
    fn in_handler(&self) -> bool {
        self.current
            .is_some_and(|def| self.program.defs[def].kind == DefKind::Handler)
    }

    /// The monomorphic type and type parameters of a definition without a declared type in this
    /// group.
    fn pending_ty(&self, def: DefId) -> Option<(Ty, Vec<TyParam>)> {
        self.pending
            .iter()
            .find(|(d, _, _)| *d == def)
            .map(|(_, ty, params)| (ty.clone(), params.clone()))
    }

    /// Checks match exhaustiveness, redundant arms, and that binding patterns always succeed
    /// (spec 6.3, 6.5). Skipped when the scrutinee type is `Ty::Error`, since another error has
    /// been reported.
    fn check_coverage(&mut self) {
        let shapes = Shapes {
            program: self.program,
            ty_table: self.ty_table,
            sigs: self.sigs,
        };
        for site in std::mem::take(&mut self.coverage) {
            match site {
                CoverageSite::Match {
                    scrutinee,
                    ty,
                    arms,
                } => {
                    let ty = self.ctx.resolve(&ty);
                    if ty == Ty::Error {
                        continue;
                    }
                    let result = shapes.check_arms(&ty, &arms);
                    if !result.missing.is_empty() {
                        self.diags.push(Diagnostic {
                            span: Some(self.program.exprs[scrutinee].span),
                            kind: DiagnosticKind::NonExhaustiveMatch {
                                missing: witnesses(&result.missing),
                            },
                        });
                    }
                    for i in result.redundant {
                        self.diags.push(Diagnostic {
                            span: Some(self.program.pats[arms[i].pat].span),
                            kind: DiagnosticKind::UnreachableArm,
                        });
                    }
                }
                CoverageSite::Escape { groups } => {
                    for (ty, arms) in groups {
                        for i in shapes.check_arms(&ty, &arms).redundant {
                            self.diags.push(Diagnostic {
                                span: Some(self.program.pats[arms[i].pat].span),
                                kind: DiagnosticKind::UnreachableArm,
                            });
                        }
                    }
                }
                CoverageSite::Binding { pat, ty } => {
                    let ty = self.ctx.resolve(&ty);
                    if ty == Ty::Error {
                        continue;
                    }
                    let arm = shapes.arm(pat, &ty, false);
                    let result = check_match(&shapes, &ty, &[arm]);
                    if !result.missing.is_empty() {
                        self.diags.push(Diagnostic {
                            span: Some(self.program.pats[pat].span),
                            kind: DiagnosticKind::RefutableBinding {
                                missing: witnesses(&result.missing),
                            },
                        });
                    }
                }
            }
        }
    }

    /// Replaces undetermined type variables in the type with fresh ones, consistently.
    fn freshen(&mut self, ty: &Ty, fresh: &mut FxHashMap<TyVar, Ty>) -> Ty {
        match self.ctx.shallow_resolve(ty) {
            Ty::Var(v) => fresh.entry(v).or_insert_with(|| self.ctx.new_var()).clone(),
            ty => ty.map_children(|child| self.freshen(child, fresh)),
        }
    }

    /// Also records which definition's body the local belongs to.
    fn set_local(&mut self, local: LocalId, ty: Ty) {
        self.locals.insert(local, ty);
        self.local_binder.insert(local, self.binder);
        if let Some(def) = self.current {
            self.local_owner.insert(local, def);
        }
    }

    /// Finishes inference: checks numeric types, generalizes definitions without declared types,
    /// and returns the types with bindings followed.
    ///
    /// Trait constraints and undetermined `Self` (E3302, E3307) are checked after numeric types,
    /// so an integer literal left open counts as Int, and before generalization, so a `Self`
    /// still open is reported rather than quantified.
    pub(crate) fn finish(mut self) -> BodyTypes {
        for r in std::mem::take(&mut self.deferred) {
            if let Some((ty, _)) = self.pending_ty(r.def) {
                let ty = self.ctx.resolve(&ty);
                let actual = keep_instance_sets(substitute(&ty, &r.args, &r.rows), &r.instance);
                self.coerce_at(r.span, &r.instance, &actual);
            }
        }
        self.check_numbers();
        self.check_known_selves();
        self.check_traits();
        // Compute and solve the error and effect sets (`body/errors.rs`, `body/effects.rs`).
        self.infer_errors();
        self.infer_effects();
        self.trait_obligations = std::mem::take(&mut self.item_obligations);
        self.check_traits();
        self.solve_errors();
        self.check_leaks();
        self.check_immediates();
        self.check_fn_op_bodies();
        self.check_suspend_after_resume();
        self.check_handler_cycles();
        self.find_bound_item_redirects();
        self.check_coverage();

        let program = self.program;
        let mut inferred = Vec::new();
        let mut generalized_rows = Vec::new();
        let mut generalized_tys = Vec::new();
        for (def, ty, params) in std::mem::take(&mut self.pending) {
            let labels = match program.item(def) {
                Some(Item::Fn(item)) => param_labels(program, &item.sig.params),
                _ => Vec::new(),
            };
            // Top level, so no environment type variables.
            let (scheme, rows, tys) =
                self.ctx
                    .generalize_bounded(&ty, params, &[], &self.immediate_vars);
            inferred.push((def, Signature { scheme, labels }));
            generalized_rows.push((def, rows));
            generalized_tys.push((def, tys));
        }
        self.check_items_determined(&generalized_tys);
        let mut with_ty_args = SparseMap::default();
        for (expr, insts) in self.item_insts.clone().iter() {
            let args = insts
                .iter()
                .map(|i| i.args.iter().map(|t| self.ctx.resolve(t)).collect())
                .collect();
            with_ty_args.insert(expr, args);
        }
        let layer_ty_args = std::mem::take(&mut self.layer_insts)
            .into_iter()
            .map(|(layer, insts)| {
                let args = insts
                    .iter()
                    .map(|i| i.args.iter().map(|t| self.ctx.resolve(t)).collect())
                    .collect();
                (layer, args)
            })
            .collect();

        let exprs = resolve_all(&mut self.ctx, &self.exprs);
        let mut errors = SparseMap::default();
        for (expr, row) in self.expr_rows.iter() {
            errors.insert(expr, self.ctx.resolve_row(row));
        }
        let mut effects = SparseMap::default();
        for (expr, row) in self.expr_effects.iter() {
            effects.insert(expr, self.ctx.resolve_row(row));
        }
        let pending_handlers = std::mem::take(&mut self.pending_handlers);
        let handler_uses = pending_handlers
            .iter()
            .map(|&(h, v, _)| (h, self.ctx.resolve_row(&Row::var(v))))
            .collect();
        let handler_fails = pending_handlers
            .iter()
            .map(|&(h, _, v)| (h, self.ctx.resolve_row(&Row::var(v))))
            .collect();
        let pats = resolve_all(&mut self.ctx, &self.pats);
        let locals = resolve_all(&mut self.ctx, &self.locals);
        let mut type_args = SparseMap::default();
        for (expr, args) in self.type_args.iter() {
            let args: Vec<Ty> = args.iter().map(|ty| self.ctx.resolve(ty)).collect();
            type_args.insert(expr, args);
        }
        let mut type_rows = SparseMap::default();
        for (expr, rows) in self.type_rows.iter() {
            let rows: Vec<Row> = rows.iter().map(|row| self.ctx.resolve_row(row)).collect();
            type_rows.insert(expr, rows);
        }
        BodyTypes {
            exprs,
            pats,
            locals,
            local_owner: self.local_owner,
            fields: self.fields,
            calls: self.calls,
            type_args,
            type_rows,
            generalized_rows,
            generalized_tys,
            pat_fields: self.pat_fields,
            diags: self.diags,
            inferred,
            errors,
            effects,
            ops: self.ops,
            members: self.member_opens,
            member_open_withs: self.member_open_withs,
            bound_item_redirects: self.bound_item_redirects,
            methods: self.methods,
            handler_uses,
            handler_fails,
            with_handlers: self.with_handlers,
            with_args: self.with_args,
            narrowings: self.narrowings,
            self_caps: self.self_caps,
            layer_args: self.layer_args,
            with_ty_args,
            layer_ty_args,
        }
    }

    /// Places `actual` where `expected` is expected; pushes a diagnostic at `span` on mismatch.
    fn coerce_at(&mut self, span: Span, expected: &Ty, actual: &Ty) {
        self.ctx.set_origin(Some(span), self.current);
        let result = self.ctx.coerce(expected, actual);
        self.ctx.set_origin(None, self.current);
        if let Err(e) = result {
            self.diags.push(Diagnostic {
                span: Some(span),
                kind: DiagnosticKind::Mismatch(e),
            });
        }
    }

    /// Infers, records and returns the expression's type.
    fn infer_expr(&mut self, expr: ExprId) -> Ty {
        let hint = self.ctx.hint();
        self.ctx.set_hint(Some(self.program.exprs[expr].span));
        let ty = self.infer_expr_kind(expr, None);
        self.ctx.set_hint(hint);
        self.exprs.insert(expr, ty.clone());
        ty
    }

    /// `expected` is the expected type. Blocks, if, match and with pass it down to their last
    /// expression or each branch (the body for with); lambdas and lists pass it to their
    /// parameters or elements first ([`Self::check_expr`]). Other expressions ignore it.
    fn infer_expr_kind(&mut self, expr: ExprId, expected: Option<&Ty>) -> Ty {
        let program = self.program;
        let e = &program.exprs[expr];
        match &e.kind {
            ExprKind::Missing => Ty::Error,
            ExprKind::Lit(lit) => self.lit_ty(lit, e.span),
            ExprKind::Unit => Ty::UNIT,
            ExprKind::Tuple(elems) => Ty::tuple(elems.iter().map(|&e| self.infer_expr(e))),
            ExprKind::Path(Res::Local(local)) => match self.locals.get(*local) {
                Some(ty) => {
                    let ty = ty.clone();
                    let ty = self.open_fails(ty);
                    self.fresh_caps(&ty)
                }
                None => Ty::Error,
            },
            ExprKind::Path(Res::Def(def)) => {
                self.check_handler_ctor(*def, e.span);
                let made_by = self.check_bound_ctor(*def, e.span);
                if program.defs[*def].kind == DefKind::Member {
                    self.check_member_open(expr, *def, e.span);
                    self.record_open_with(expr, *def, None);
                }
                let (ty, args, rows) = self.instantiate_def(*def, e.span);
                if let Some(cap) = made_by {
                    self.record_maker(&ty, cap);
                }
                let ty = match program.member_labels.get(expr) {
                    Some(&label) => through_label(ty, label),
                    None => ty,
                };
                let ty = if program.defs[*def].kind == DefKind::Member {
                    self.opened_in_init(expr, *def, ty)
                } else {
                    ty
                };
                if !args.is_empty() {
                    self.type_args.insert(expr, args);
                }
                if !rows.is_empty() {
                    self.type_rows.insert(expr, rows);
                }
                self.open_fails(ty)
            }
            ExprKind::Path(Res::Err) => Ty::Error,
            ExprKind::Block(block) => self.infer_block(block, e.span, expected),
            ExprKind::Call { callee, args } => self.infer_call(expr, *callee, args, e.span),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.infer_if(*cond, *then_branch, *else_branch, e.span, expected),
            ExprKind::Match { scrutinee, arms } => self.infer_match(*scrutinee, arms, expected),
            ExprKind::Binary { op, lhs, rhs } => self.infer_binary(expr, *op, *lhs, *rhs, e.span),
            ExprKind::Unary { op, operand } => self.infer_unary(*op, *operand, e.span),
            ExprKind::Lambda { params, body } => {
                self.infer_lambda(expr, params, *body, e.span, expected)
            }
            ExprKind::List { elems, rest } => self.infer_list(elems, *rest, e.span, expected),
            ExprKind::Field { base, name } => self.infer_field(expr, *base, name),
            ExprKind::Interpolated(parts) => {
                for part in parts {
                    if let StrPart::Interp(expr) = part {
                        let ty = self.infer_expr(*expr);
                        let span = program.exprs[*expr].span;
                        self.require_prelude_trait(&ty, "Show", span);
                    }
                }
                Ty::STRING
            }
            ExprKind::Assert(cond) => {
                self.check_expr(*cond, &Ty::BOOL);
                Ty::UNIT
            }
            ExprKind::Update { ctor, fields, base } => {
                self.infer_update(expr, *ctor, fields, *base)
            }
            // `fail e` is Never. The tag of e's type is added to E when the body's E is computed
            // (spec 7.2).
            ExprKind::Fail(inner) => {
                self.infer_expr(*inner);
                Ty::Never
            }
            ExprKind::Escape { expr: inner, arms } => self.infer_escape(*inner, arms),
            // `use X` is X's capability value (spec 8.2). X is added to R when the body's R is
            // computed. A host effect and External have no operations; only the host provides
            // them (`[effect.host]`, `[effect.external]`).
            ExprKind::Use {
                effect: Res::Def(effect),
                ..
            } if program.is_provided_by_host(*effect) => {
                self.diags.push(Diagnostic {
                    span: Some(e.span),
                    kind: DiagnosticKind::UseHostEffect {
                        effect: program.defs[*effect].name.clone(),
                        external: program.is_external_effect(*effect),
                    },
                });
                Ty::Error
            }
            // A built-in effect's type arguments may be omitted (`use TaskGroup`, `[task.group]`); they are
            // inferred.
            ExprKind::Use {
                effect: Res::Def(effect),
                args,
            } if args.is_empty() && is_intrinsic_effect(program, *effect) => {
                let params = effect_params(program, *effect).len();
                let args = (0..params)
                    .map(|i| match self.ty_table.set_kind(*effect, i) {
                        Some(kind) => Ty::Set(kind, Row::var(self.ctx.new_row_var())),
                        None => self.ctx.new_var(),
                    })
                    .collect();
                let tag = Tag::with_args(*effect, args);
                let caps = self.use_caps(expr, &tag);
                Ty::cap([tag], caps)
            }
            // Effects with type parameters need their type arguments (`use State[Int]`, spec 8.1);
            // omitting them is E4008.
            ExprKind::Use {
                effect: Res::Def(effect),
                args,
            } => {
                let scope = Scope {
                    params: &self.type_params,
                    kinds: &self.param_kinds,
                    omitted: Omitted::Empty,
                    sets: true,
                    self_ty: self.self_ty.as_ref(),
                    result_caps: None,
                    quant: None,
                };
                match self.ty_table.effect_tag(
                    program,
                    &scope,
                    *effect,
                    args,
                    Some(e.span),
                    None,
                    &mut self.diags,
                ) {
                    Some(tag) => {
                        // The capability sets of capability types in the type arguments are
                        // inferred (`use State[use Name]`, `[type.rules]`).
                        let tag = tag.map_args(|a| self.infer_written_caps(a));
                        let caps = self.use_caps(expr, &tag);
                        Ty::cap([tag], caps)
                    }
                    None => Ty::Error,
                }
            }
            ExprKind::Use { .. } => Ty::Error,
            ExprKind::With { handlers, body } => {
                self.infer_with_args(expr, handlers);
                let insts = self.item_insts(expr, handlers);
                let provides =
                    crate::layer::with_entries(program, self.sigs, self.ty_table, handlers, &insts)
                        .into_iter()
                        .filter(|e| e.named)
                        .map(|e| e.effect)
                        .collect();
                self.name_providers.push(Some((expr, provides)));
                self.ctx.scopes.enter(expr);
                let in_init = self.in_init.is_some();
                if in_init {
                    self.init_withs.push(expr);
                }
                let ty = match expected {
                    Some(expected) => {
                        self.check_expr(*body, expected);
                        expected.clone()
                    }
                    None => self.infer_expr(*body),
                };
                if in_init {
                    self.init_withs.pop();
                }
                self.ctx.scopes.exit();
                self.name_providers.pop();
                ty
            }
        }
    }

    /// `()` if the block ends with a binding or is empty. With an expected type, it is passed to
    /// the last expression and becomes the block's type.
    fn infer_block(&mut self, block: &Block, span: Span, expected: Option<&Ty>) -> Ty {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { pat, ty, value } => {
                    let bound_ty = match ty {
                        Some(ty) => {
                            let expected = self.lower_type(*ty);
                            self.check_expr(*value, &expected);
                            expected
                        }
                        None => self.infer_expr(*value),
                    };
                    if let PatKind::Bind(local) = self.program.pats[*pat].kind {
                        self.let_values.insert(local, *value);
                    }
                    self.bind_pat(*pat, &bound_ty);
                    self.coverage.push(CoverageSite::Binding {
                        pat: *pat,
                        ty: bound_ty,
                    });
                }
                Stmt::LocalFn(f) => self.infer_local_fn(f),
                Stmt::Expr(expr) => {
                    self.infer_expr(*expr);
                }
            }
        }
        match (block.tail, expected) {
            (Some(expr), Some(expected)) => {
                self.check_expr(expr, expected);
                expected.clone()
            }
            (Some(expr), None) => self.infer_expr(expr),
            (None, expected) => self.unit_at(span, expected),
        }
    }

    /// The type of a reference to a function value, with fresh set variables added to E and R. A
    /// function with a smaller E (R) may be used as one with a larger E (R), so functions with
    /// different E can be combined in merged branches or list elements.
    fn open_fails(&mut self, ty: Ty) -> Ty {
        match self.ctx.shallow_resolve(&ty) {
            Ty::Fn(f) => {
                let mut fails = f.fails.clone();
                fails.insert(crate::RowElem::Var(self.ctx.new_row_var()));
                let mut uses = f.uses.clone();
                uses.insert(crate::RowElem::Var(self.ctx.new_row_var()));
                Ty::func_sets(f.params, f.ret, fails, uses)
            }
            _ => ty,
        }
    }

    /// With an expected type, `()` is placed at it.
    fn unit_at(&mut self, span: Span, expected: Option<&Ty>) -> Ty {
        if let Some(expected) = expected {
            self.coerce_at(span, expected, &Ty::UNIT);
        }
        Ty::UNIT
    }

    /// Types like [`Self::infer_def_ref`], also returning the instantiated type arguments and sets
    /// (both in the declaration's type parameter order; sets are empty at positions that are not
    /// set parameters). References without written type parameters (including monomorphic ones)
    /// and a definition's references to itself are not instantiated (both empty).
    fn instantiate_def(&mut self, def: DefId, span: Span) -> (Ty, Vec<Ty>, Vec<Row>) {
        let sigs = self.sigs;
        if let Some(sig) = sigs.get(def) {
            let (tys, rows) = self.ctx.fresh_args(&sig.scheme.params);
            self.require_immediate(def, &sig.scheme.params, &rows, span);
            self.require_traits(&sig.scheme.params, &tys, span);
            self.require_known_self(def, &sig.scheme, &tys, span);
            let ty = substitute(&sig.scheme.ty, &tys, &rows);
            return (self.anchor_names(&ty), tys, rows);
        }
        if let Some((ty, params)) = self.pending_ty(def) {
            if self.current == Some(def) || params.is_empty() {
                return (ty, Vec::new(), Vec::new());
            }
            // Written type parameters (`Ty::Bound(i)`) and invisible set type parameters only mean
            // something inside their definition, so references from other definitions replace
            // them with fresh type and set variables. Undetermined parts (such as the return type)
            // may later turn out to be that definition's type parameters, so they get fresh type
            // variables here and are checked once the group is inferred.
            let ty = self.ctx.resolve(&ty);
            let (args, rows) = self.ctx.fresh_args(&params);
            self.require_immediate(def, &params, &rows, span);
            self.require_traits(&params, &args, span);
            let mut fresh = FxHashMap::default();
            let instance = self.freshen(&substitute(&ty, &args, &rows), &mut fresh);
            let instance = self.instance_sets(&ty, instance, &args, &rows);
            self.deferred.push(DeferredRef {
                def,
                args: args.clone(),
                rows: rows.clone(),
                instance: instance.clone(),
                span,
            });
            return (instance, args, rows);
        }
        // Kinds of definitions not typed yet (effect operations referred to by name, ...).
        self.diags.push(Diagnostic {
            span: Some(span),
            kind: DiagnosticKind::Unsupported {
                what: "reference to this kind of definition",
            },
        });
        (Ty::Error, Vec::new(), Vec::new())
    }

    /// Remembers the trait constraints of the type parameters `params` instantiated with `tys`
    /// at `span`, to check once types are settled.
    fn require_traits(&mut self, params: &[TyParam], tys: &[Ty], span: Span) {
        for (p, ty) in params.iter().zip(tys) {
            for &trait_def in &p.traits {
                self.trait_obligations.push(TraitObligation {
                    ty: ty.clone(),
                    trait_def,
                    span,
                    owner: self.current,
                    float_ok: false,
                });
            }
        }
    }

    /// Without else, the body is `()` or `Never` and the whole is `()` (spec 6.4). With an
    /// expected type and an else, both branches get the expected type.
    fn infer_if(
        &mut self,
        cond: ExprId,
        then_branch: ExprId,
        else_branch: Option<ExprId>,
        span: Span,
        expected: Option<&Ty>,
    ) -> Ty {
        self.check_expr(cond, &Ty::BOOL);

        match (else_branch, expected) {
            (Some(else_branch), Some(expected)) => {
                self.check_expr(then_branch, expected);
                self.check_expr(else_branch, expected);
                expected.clone()
            }
            (Some(else_branch), None) => {
                let then_ty = self.infer_expr(then_branch);
                let else_ty = self.infer_expr(else_branch);
                let span = self.program.exprs[else_branch].span;
                self.join_at(span, &then_ty, &else_ty)
            }
            (None, expected) => {
                self.check_expr(then_branch, &Ty::UNIT);
                self.unit_at(span, expected)
            }
        }
    }

    /// Binary operators (spec 6.6). Pipes are read as calls (spec 6.7).
    fn infer_binary(
        &mut self,
        expr: ExprId,
        op: BinaryOp,
        lhs: ExprId,
        rhs: ExprId,
        span: Span,
    ) -> Ty {
        match op {
            // Arithmetic: both sides have the same numeric type.
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => {
                let ty = self.infer_expr(lhs);
                self.check_expr(rhs, &ty);
                self.require_numeric(&ty, span);
                ty
            }
            // Comparison: both sides have the same type, which satisfies Eq (`==`, `!=`) or Ord
            // (spec 10.3). Float's ordering operators are built in (spec 10.6).
            BinaryOp::Eq
            | BinaryOp::Ne
            | BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge => {
                let ty = self.infer_expr(lhs);
                self.check_expr(rhs, &ty);
                let name = if matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
                    "Eq"
                } else {
                    "Ord"
                };
                self.require_prelude_trait(&ty, name, span);
                Ty::BOOL
            }
            // Logical: both sides are Bool.
            BinaryOp::And | BinaryOp::Or => {
                self.check_expr(lhs, &Ty::BOOL);
                self.check_expr(rhs, &Ty::BOOL);
                Ty::BOOL
            }
            BinaryOp::Pipe => self.infer_pipe(expr, lhs, rhs, span),
        }
    }

    /// Infers `x |> f(a)` as `f(x, a)` and `x |> f` as `f(x)` (spec 6.7). The reordered arguments
    /// (including the left side) are recorded on the pipe expression `expr`. The callee is the
    /// right side's callee if it is a call, otherwise the right side itself.
    fn infer_pipe(&mut self, expr: ExprId, lhs: ExprId, rhs: ExprId, span: Span) -> Ty {
        let program = self.program;
        let piped = Arg {
            label: None,
            value: lhs,
            punned: false,
        };
        let ty = match &program.exprs[rhs].kind {
            ExprKind::Call { callee, args } => {
                let args: Vec<Arg> = std::iter::once(piped).chain(args.iter().cloned()).collect();
                self.infer_call(expr, *callee, &args, span)
            }
            _ => self.infer_call(expr, rhs, &[piped], span),
        };
        // Also record the rewritten call's type on the right side's call expression.
        if matches!(program.exprs[rhs].kind, ExprKind::Call { .. }) {
            self.exprs.insert(rhs, ty.clone());
        }
        ty
    }

    /// All elements have the same type, and `..rest` is a list of that type (spec 2.4, 17.5). If
    /// the expected type is a list, its element type is passed to each element first.
    fn infer_list(
        &mut self,
        elems: &[ExprId],
        rest: Option<ExprId>,
        span: Span,
        expected: Option<&Ty>,
    ) -> Ty {
        let from_context = match expected.map(|ty| self.ctx.shallow_resolve(ty)) {
            Some(Ty::List(elem)) => Some(*elem),
            _ => None,
        };
        let elem_ty = match &from_context {
            Some(elem) => elem.clone(),
            None => self.ctx.new_var(),
        };
        for &elem in elems {
            self.check_expr(elem, &elem_ty);
        }
        let ty = Ty::list(elem_ty);
        if let Some(rest) = rest {
            self.check_expr(rest, &ty);
        }
        if let (Some(expected), None) = (expected, &from_context) {
            self.coerce_at(span, expected, &ty);
        }
        ty
    }

    /// The type of `x.name` when it is not called: a field, looked up in the declaration of the
    /// base type (type or error), or an operation of a capability value. The base type must be
    /// known by now; it is not inferred backwards from the name.
    fn infer_field(&mut self, expr: ExprId, base: ExprId, name: &Label) -> Ty {
        let base_ty = self.infer_expr(base);
        match self.infer_dot(expr, base, &base_ty, name, false) {
            methods::Dot::Value(ty) => ty,
            methods::Dot::Method(_) => unreachable!("a method is found only for a call"),
        }
    }

    /// `!` takes Bool, `-` a numeric type (spec 6.6).
    fn infer_unary(&mut self, op: UnaryOp, operand: ExprId, span: Span) -> Ty {
        match op {
            UnaryOp::Not => {
                self.check_expr(operand, &Ty::BOOL);
                Ty::BOOL
            }
            UnaryOp::Neg => {
                let ty = self.infer_expr(operand);
                self.require_numeric(&ty, span);
                ty
            }
        }
    }

    /// Each arm's pattern gets the scrutinee type and the arm bodies' types are joined, starting
    /// from `Never`; a match without arms is `Never`. With an expected type, it is passed to each
    /// arm body.
    fn infer_match(&mut self, scrutinee: ExprId, arms: &[Arm], expected: Option<&Ty>) -> Ty {
        let scrutinee_ty = self.infer_expr(scrutinee);
        self.coverage.push(CoverageSite::Match {
            scrutinee,
            ty: scrutinee_ty.clone(),
            arms: arms.to_vec(),
        });

        let mut result = Ty::Never;

        for arm in arms {
            self.bind_pat(arm.pat, &scrutinee_ty);

            if let Some(guard) = arm.guard {
                self.check_expr(guard, &Ty::BOOL);
            }

            match expected {
                Some(expected) => {
                    self.check_expr(arm.body, expected);
                    result = expected.clone();
                }
                None => {
                    let body_ty = self.infer_expr(arm.body);
                    let span = self.program.exprs[arm.body].span;
                    result = self.join_at(span, &result, &body_ty);
                }
            }
        }

        result
    }

    /// The type of `escape` (`[error.escape]`). Arm patterns are error constructors matched against
    /// values of that type, or `_`. Arm results have the inner expression's type or Never, and the
    /// whole has the inner expression's type. Caught tags and E are handled when the body's E is
    /// computed (`body/errors.rs`). Arms need not be exhaustive.
    ///
    /// Every arm below an arm `_` without a guard is unreachable (W3101), whatever the E of the
    /// inner expression. Such arms are reported here and left out of the per-error groups, so an arm
    /// both below `_` and covered by an arm of its error is reported once.
    fn infer_escape(&mut self, inner: ExprId, arms: &[Arm]) -> Ty {
        let program = self.program;
        let inner_ty = self.infer_expr(inner);
        let mut groups: Vec<(DefId, Ty, Vec<Arm>)> = Vec::new();
        let mut result = inner_ty;
        let mut below_wildcard = false;
        for arm in arms {
            let p = &program.pats[arm.pat];
            if below_wildcard {
                self.diags.push(Diagnostic {
                    span: Some(p.span),
                    kind: DiagnosticKind::UnreachableArm,
                });
            }
            match &p.kind {
                PatKind::Ctor {
                    res: Res::Def(def), ..
                } if program.defs[*def].kind == DefKind::Error => {
                    // The sets of the error's fields come from the tag in E (`body/errors.rs`).
                    let ty = self.ty_table.self_ty(*def).unwrap_or(Ty::Error);
                    let ty = self.fresh_hidden_caps(*def, &ty);
                    self.bind_pat(arm.pat, &ty);
                    if !below_wildcard {
                        match groups.iter_mut().find(|(d, _, _)| d == def) {
                            Some((_, _, group)) => group.push(arm.clone()),
                            None => groups.push((*def, ty, vec![arm.clone()])),
                        }
                    }
                }
                // `_` binds nothing, so its type is never read.
                PatKind::Wild => {
                    self.bind_pat(arm.pat, &Ty::Error);
                    match arm.guard {
                        Some(guard) => self.diags.push(Diagnostic {
                            span: Some(program.exprs[guard].span),
                            kind: DiagnosticKind::EscapeWildcardGuard,
                        }),
                        None => below_wildcard = true,
                    }
                }
                // Name resolution has reported non-error constructors.
                PatKind::Ctor { .. } | PatKind::Missing => self.bind_pat(arm.pat, &Ty::Error),
                _ => {
                    self.diags.push(Diagnostic {
                        span: Some(p.span),
                        kind: DiagnosticKind::EscapeArmNotError,
                    });
                    self.bind_pat(arm.pat, &Ty::Error);
                }
            }
            if let Some(guard) = arm.guard {
                self.check_expr(guard, &Ty::BOOL);
            }
            let body_ty = self.infer_expr(arm.body);
            let span = program.exprs[arm.body].span;
            result = self.join_at(span, &result, &body_ty);
        }
        self.coverage.push(CoverageSite::Escape {
            groups: groups.into_iter().map(|(_, ty, arms)| (ty, arms)).collect(),
        });
        result
    }

    /// Infers the expression and places it at the expected type.
    ///
    /// Blocks, if, match and with pass the expected type down to their last expression or each
    /// branch, so a mismatch points at the innermost differing expression. Lambdas and lists pass
    /// it to their parameters or elements first. Other expressions are inferred, then checked.
    fn check_expr(&mut self, expr: ExprId, expected: &Ty) {
        let program = self.program;
        let e = &program.exprs[expr];
        match e.kind {
            ExprKind::Block(_)
            | ExprKind::If { .. }
            | ExprKind::Match { .. }
            | ExprKind::Lambda { .. }
            | ExprKind::List { .. }
            | ExprKind::With { .. } => {
                let ty = self.infer_expr_kind(expr, Some(expected));
                self.exprs.insert(expr, ty);
            }
            _ => {
                let actual = self.infer_expr(expr);
                self.coerce_at(e.span, expected, &actual);
            }
        }
    }

    /// Converts a type annotation inside a body.
    ///
    /// An omitted `fails` on function types in binding annotations and lambda parameter types is
    /// inferred from the body (spec 9.2).
    fn lower_type(&mut self, ty: TypeId) -> Ty {
        let mut scope = Scope {
            params: &self.type_params,
            kinds: &self.param_kinds,
            omitted: Omitted::Fresh(&mut self.ctx),
            sets: true,
            self_ty: self.self_ty.as_ref(),
            result_caps: None,
            quant: None,
        };
        self.ty_table
            .lower_in(self.program, &mut scope, ty, true, &mut self.diags)
    }

    /// The common type of two branches. On mismatch, pushes a diagnostic at `span` and takes `a`
    /// as the overall type.
    fn join_at(&mut self, span: Span, a: &Ty, b: &Ty) -> Ty {
        self.ctx.set_origin(Some(span), self.current);
        let result = self.ctx.join(a, b);
        self.ctx.set_origin(None, self.current);
        match result {
            Ok(ty) => ty,
            Err(e) => {
                self.diags.push(Diagnostic {
                    span: Some(span),
                    kind: DiagnosticKind::Mismatch(e),
                });
                a.clone()
            }
        }
    }

    /// Integers get an Int-or-Int64 type variable, decided by context (spec 2.4).
    fn lit_ty(&mut self, lit: &Literal, span: Span) -> Ty {
        match lit {
            Literal::Int(text) => {
                let ty = self.ctx.new_int_var();
                self.numbers.push(NumConstraint {
                    ty: ty.clone(),
                    span,
                    kind: NumKind::Integer,
                    literal: Some(text.clone()),
                });
                ty
            }
            Literal::Float(_) => Ty::FLOAT,
            Literal::String(_) => Ty::STRING,
        }
    }

    /// Requires `ty` to be Int, Int64 or Float (arithmetic operators, unary minus). Checked once
    /// the body is inferred.
    fn require_numeric(&mut self, ty: &Ty, span: Span) {
        self.numbers.push(NumConstraint {
            ty: ty.clone(),
            span,
            kind: NumKind::Numeric,
            literal: None,
        });
    }

    /// Checks numeric type constraints. Undetermined type variables become Int (spec 2.4). Integer
    /// literals are also checked to fit the decided type.
    fn check_numbers(&mut self) {
        for c in std::mem::take(&mut self.numbers) {
            let ty = match self.ctx.shallow_resolve(&c.ty) {
                Ty::Var(_) => {
                    // A number constrained to nothing is Int; unification always succeeds.
                    let _ = self.ctx.unify(&Ty::INT, &c.ty);
                    Ty::INT
                }
                ty => ty,
            };
            let ok = match (&ty, c.kind) {
                (Ty::Error | Ty::Never, _) => continue,
                (Ty::Prim(Prim::Int | Prim::Int64), _) => true,
                (Ty::Prim(Prim::Float), NumKind::Numeric) => true,
                _ => false,
            };
            if !ok {
                self.diags.push(Diagnostic {
                    span: Some(c.span),
                    kind: DiagnosticKind::NotNumeric {
                        found: self.ctx.resolve(&ty),
                        integer: c.kind == NumKind::Integer,
                    },
                });
                continue;
            }
            if let Some(literal) = c.literal
                && !int_literal_fits(&literal, &ty)
            {
                self.diags.push(Diagnostic {
                    span: Some(c.span),
                    kind: DiagnosticKind::IntLiteralOutOfRange { literal, ty },
                });
            }
        }
    }
}

/// The result of reordering arguments by parameter position ([`BodyInfer::assign_args`]).
struct Assigned<T> {
    /// Arguments per position; `None` if unfilled.
    slots: Vec<Option<T>>,
    /// Arguments that could not be placed. Inferred only to record their types, not checked.
    extra: Vec<T>,
    /// Whether a label error was reported; if so, count errors are not reported on top.
    reported: bool,
    /// Whether there were more positional arguments than parameters.
    too_many: bool,
}

impl<T> Assigned<T> {
    fn has_missing(&self) -> bool {
        self.slots.iter().any(Option::is_none)
    }
}

/// Follows unification bindings for every type in the table.
fn resolve_all<T>(ctx: &mut InferCtx, map: &SparseMap<Idx<T>, Ty>) -> SparseMap<Idx<T>, Ty> {
    let mut out = SparseMap::default();
    for (id, ty) in map.iter() {
        out.insert(id, ctx.resolve(ty));
    }
    out
}

struct NumConstraint {
    ty: Ty,
    /// The diagnostic span: the literal or the operator expression.
    span: Span,
    kind: NumKind,
    /// For integer literals, the digits (without `_`; a `0x` or `0b` prefix is kept).
    literal: Option<SmolStr>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NumKind {
    /// Int or Int64, for integer literals.
    Integer,
    /// Int, Int64 or Float, for arithmetic operators and unary minus.
    Numeric,
}

/// Whether an integer literal fits the type. In expressions negative numbers are unary minus, so
/// only a pattern's literal can be negative (spec 6.5). Not reinterpreted as two's complement
/// (`0xFFFF_FFFF` does not fit Int, spec 2.4).
fn int_literal_fits(literal: &str, ty: &Ty) -> bool {
    let range = match ty {
        Ty::Prim(Prim::Int) => i32::MIN as i128..=i32::MAX as i128,
        Ty::Prim(Prim::Int64) => i64::MIN as i128..=i64::MAX as i128,
        _ => return true,
    };
    int_literal_value(literal).is_some_and(|n| range.contains(&n))
}

enum FieldTy {
    /// The field index and type.
    Found(usize, Ty),
    /// The type has fields, but none with that name.
    NoSuchField,
    /// A type without fields (an enum, or an `@external` type without fields).
    NotARecord,
}

/// An effect set required to be `Immediate` (spec 9.5), checked after sets are solved
/// (`body/suspend.rs`).
struct ImmediateObligation {
    /// The instantiated set.
    row: Row,
    /// The span of the reference.
    span: Span,
    /// The referenced definition (`RowElem::Bound` in `row` refers to its type parameters).
    owner: Option<DefId>,
    source: crate::diagnostic::ImmediateSource,
    /// For a callback of an external function: whether its parameter is `@borrowed`, so that
    /// capabilities may be in its R (`[ext.boundary]`).
    borrowed: bool,
    /// For a callback of an external function: the External of the module that declares it,
    /// which may be in its R (`[effect.external]`).
    allowed_external: Option<DefId>,
}

/// A type that must satisfy a trait: the instantiation of a constrained type parameter at a
/// reference (`[A: Ord]`, spec 10.3).
/// `ty` with the set variables left out of its function types' E and R, for messages.
/// The type of opening a handler member through the label `label` (`Audit.Open`,
/// `[effect.label]`): it requires the label, not its base.
fn through_label(ty: Ty, label: DefId) -> Ty {
    match ty {
        Ty::Fn(f) => Ty::func_sets(f.params, f.ret, f.fails, Row::of_tag(Tag::new(label))),
        ty => ty,
    }
}

struct TraitObligation {
    ty: Ty,
    trait_def: DefId,
    /// The span of the reference.
    span: Span,
    /// The definition whose body has the reference; `Ty::Bound(i)` in `ty` is its type
    /// parameter.
    owner: Option<DefId>,
    /// A Float passes: the operand of a comparison, which is the built-in IEEE 754 operator on
    /// Float rather than Ord (spec 10.6).
    float_ok: bool,
}

/// The `Self` of a trait function determined only by its result type (E3307).
struct SelfObligation {
    ty: Ty,
    trait_def: DefId,
    /// The span of the reference.
    span: Span,
}

/// A reference within the group to a definition without a declared type but with written type
/// parameters.
struct DeferredRef {
    def: DefId,
    /// Type variables placed for the type parameters.
    args: Vec<Ty>,
    /// Set variables placed for the set type parameters.
    rows: Vec<Row>,
    /// The type given to the reference expression.
    instance: Ty,
    span: Span,
}

/// The inferred E or R (`inferred`) of a definition of the group referred to from another
/// definition, and the set variable (`instance`) that stands for it in the reference's type. Once
/// solved, `inferred` with `args` and `rows` placed is contained in `instance`
/// (`body/errors.rs`).
#[derive(Clone)]
pub(crate) struct InstanceSet {
    pub(crate) inferred: Row,
    pub(crate) instance: RowVar,
    pub(crate) kind: SetKind,
    pub(crate) args: Vec<Ty>,
    pub(crate) rows: Vec<Row>,
}

/// `actual` with the E and R that [`BodyInfer::instance_sets`] replaced in `instance` taken from
/// `instance`: those are related through [`InstanceSet`] instead, once solved.
fn keep_instance_sets(actual: Ty, instance: &Ty) -> Ty {
    let (Ty::Fn(a), Ty::Fn(i)) = (&actual, instance) else {
        return actual;
    };
    let pick = |a: &Row, i: &Row| {
        if a.vars().next().is_some() {
            i.clone()
        } else {
            a.clone()
        }
    };
    Ty::func_sets(
        a.params.clone(),
        a.ret.clone(),
        pick(&a.fails, &i.fails),
        pick(&a.uses, &i.uses),
    )
}

/// Places where coverage is checked.
enum CoverageSite {
    Match {
        scrutinee: ExprId,
        ty: Ty,
        arms: Vec<Arm>,
    },
    /// A binding `pat = value`, whose pattern must always succeed (spec 6.3).
    Binding { pat: PatId, ty: Ty },
    /// `escape` arms. Grouped by the caught error type; only redundant arms are checked
    /// (exhaustiveness is not required).
    Escape { groups: Vec<(Ty, Vec<Arm>)> },
}

/// Turns missing-value examples into strings.
fn witnesses(missing: &[Witness]) -> Vec<SmolStr> {
    missing
        .iter()
        .map(|w| SmolStr::new(w.to_string()))
        .collect()
}

/// The effects of a capability type as named in a diagnostic: `Query + Execute`.
pub(crate) fn cap_effects_name(program: &Program, tags: &[Tag]) -> SmolStr {
    let names: Vec<&str> = tags
        .iter()
        .map(|t| program.defs[t.def].name.as_str())
        .collect();
    SmolStr::new(names.join(" + "))
}
