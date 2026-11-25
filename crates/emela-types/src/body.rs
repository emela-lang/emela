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
    Arg, Arm, BinaryOp, Block, DefId, DefKind, DefModule, ExprId, ExprKind, FnSig, HandlerItem,
    Item, Label, ListRest, Literal, LocalFn, LocalId, LocalKind, Param, PatArg, PatId, PatKind,
    Program, Res, Span, Stmt, StrPart, SupplyArgs, TypeId, UnaryOp, WithItem, int_literal_value,
};
use la_arena::{ArenaMap, Idx};
use rustc_hash::{FxHashMap, FxHashSet};
use smol_str::SmolStr;

use crate::FnTy;
use crate::builtin::Builtins;
use crate::coverage::Shapes;
use crate::diagnostic::{Diagnostic, DiagnosticKind, EffectContext};
use crate::exhaustiveness::{Witness, check_match};
use crate::infer::{InferCtx, Origin, SetKind, substitute, substitute_bound};
use crate::lower::{
    Omitted, Scope, TyTable, effect_params, has_omitted_fails, is_intrinsic_effect,
};
use crate::row::{CapElem, CapId, Row, RowElem, RowVar, Tag};
use crate::signature::{FnParams, Signature, Signatures, param_labels};
use crate::traits::Traits;
use crate::ty::{ParamKind, Prim, Ty, TyParam, TyVar};

mod bound;
mod capabilities;
mod effects;
mod errors;
mod items;
mod methods;
mod suspend;

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
    /// The `use E` expressions (W0403) and the effect aliases written as a value (E0453) for
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
    /// Whether it implements a suspend fn. fn operation bodies cannot suspend (E0417).
    suspend: bool,
    /// The `resume` parameter of a suspend fn implementation, for E0461.
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
    /// body is checked against it (E0318); otherwise it is a set variable.
    fails_declared: bool,
    /// Whether `fails` is written, for the message of E0318.
    fails_written: bool,
    /// Whether `uses` is written. Then the R of the body is checked against it (E0402).
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
    /// in its own init (E0418).
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
    /// Where each capability first entered each set variable, for E0455 (`[effect.escape]`).
    cap_provenance: FxHashMap<(RowVar, crate::row::CapId), Option<Span>>,
    /// `escape` inner expressions, the errors their arms catch and their arm `_`, for the W0302
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
    /// (E0332 is reported with the impl).
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

    /// Infers a handler's init, release and operation bodies (spec 8.4).
    ///
    /// - init's value is the handler value. If the handler has fields, init is required (E0419).
    /// - `self` in release and operation bodies is the handler value. release's value is dropped.
    /// - Operation parameter and return types come from the effect operation's declaration. A
    ///   suspend fn implementation takes `resume` as its last parameter: a function taking a value
    ///   of the operation's return type and returning `()`. The body's value is `()`.
    /// - Handlers for a host effect cannot be written (`[effect.host]`).
    fn infer_handler(&mut self, def: DefId, item: &HandlerItem) {
        let program = self.program;
        // The runtime implements a built-in handler (`Deferred`, `[kw.annotation]`): it has no
        // init, release or operations to check.
        if item.is_intrinsic {
            return;
        }
        let span = program.defs[def].span;
        // The effects listed after `:` (`[effect.handler]`); one that did not resolve leaves the
        // handler unchecked, as name resolution has reported it.
        let mut effects: Vec<DefId> = Vec::with_capacity(item.effects.len());
        for e in &item.effects {
            let Res::Def(effect) = e.res else {
                return;
            };
            if program.is_host_effect(effect) {
                self.diags.push(Diagnostic {
                    span,
                    kind: DiagnosticKind::HostEffectHandler {
                        handler: program.defs[def].name.clone(),
                        effect: program.defs[effect].name.clone(),
                    },
                });
                return;
            }
            // Only a built-in handler, or the scope function, provides a built-in effect
            // (`Task.Group`, `[task.group]`; `Deferred`, `[task.deferred]`).
            if is_intrinsic_effect(program, effect) {
                let name = program.defs[effect].name.clone();
                self.diags.push(Diagnostic {
                    span,
                    kind: DiagnosticKind::IntrinsicHandler {
                        handler: program.defs[def].name.clone(),
                        provider: self
                            .builtins
                            .provider(effect)
                            .cloned()
                            .unwrap_or(name.clone()),
                        effect: name,
                    },
                });
                return;
            }
            // A handler of a label is E0433 (signatures); its operations are not looked up.
            if crate::signature::is_label(program, effect) {
                return;
            }
            if !effects.contains(&effect) {
                effects.push(effect);
            }
        }
        if effects.is_empty() {
            return;
        }
        let handler_ty = self.ty_table.self_ty(def).unwrap_or(Ty::Error);
        match item.init {
            Some(init) => {
                let params = self.sigs.init_params(def).map(|p| p.tys.clone());
                for (i, p) in item.init_params.iter().enumerate() {
                    let ty = params
                        .as_ref()
                        .and_then(|tys| tys.get(i).cloned())
                        .unwrap_or(Ty::Error);
                    self.set_local(p.local, ty);
                }
                self.in_init = Some(def);
                // The sets of the fields are decided by what the init gives them; the fields
                // given cannot record a capability of a with inside the init (E0455).
                let made = self.fresh_hidden_caps(def, &handler_ty);
                self.check_expr(init, &made);
                self.in_init = None;
                self.handler_parts.push(HandlerPart {
                    handler: def,
                    body: init,
                    release: false,
                });
            }
            None if !item.fields.is_empty() => self.diags.push(Diagnostic {
                span,
                kind: DiagnosticKind::MissingInit {
                    handler: program.defs[def].name.clone(),
                },
            }),
            None => {}
        }
        if let Some((self_local, body)) = item.release {
            self.set_local(self_local, handler_ty.clone());
            self.infer_expr(body);
            self.handler_parts.push(HandlerPart {
                handler: def,
                body,
                release: true,
            });
        }
        // The operations of every listed effect, in the order of the effects
        // (`[effect.handler]`). A missing handler member is E0404 too, listed after the
        // operations.
        let inner_member = |inner: DefId| match program.item(inner) {
            Some(Item::Handler(h)) => h.member,
            _ => None,
        };
        let declared = |members: bool| -> Vec<DefId> {
            effects
                .iter()
                .flat_map(|&effect| match program.item(effect) {
                    Some(Item::Effect(e)) if members => e.members.as_slice(),
                    Some(Item::Effect(e)) => e.ops.as_slice(),
                    _ => &[],
                })
                .copied()
                .collect()
        };
        let (ops, members) = (declared(false), declared(true));
        // An operation whose name another listed effect has too is E0450
        // (`check_listed_effect`), not missing.
        let missing: Vec<SmolStr> = ops
            .iter()
            .filter(|&&op| !item.ops.iter().any(|h| h.op == Res::Def(op)))
            .map(|&op| program.defs[op].name.clone())
            .filter(|name| {
                ops.iter()
                    .filter(|&&o| program.defs[o].name == *name)
                    .count()
                    == 1
            })
            .collect();
        let missing_members: Vec<SmolStr> = members
            .iter()
            .filter(|&&m| {
                !item
                    .inner
                    .iter()
                    .any(|&i| inner_member(i) == Some(Res::Def(m)))
            })
            .map(|&m| program.defs[m].name.clone())
            .collect();
        for (k, &inner) in item.inner.iter().enumerate() {
            if let Some(Res::Def(m)) = inner_member(inner)
                && item.inner[..k]
                    .iter()
                    .any(|&i| inner_member(i) == Some(Res::Def(m)))
            {
                self.diags.push(Diagnostic {
                    span: program.defs[inner].span,
                    kind: DiagnosticKind::DuplicateHandlerOp {
                        op: program.defs[m].name.clone(),
                    },
                });
            }
        }
        if let Some(Res::Def(member)) = item.member {
            self.check_inner_handler(def, member, item);
        }
        if !missing.is_empty() || !missing_members.is_empty() {
            self.diags.push(Diagnostic {
                span,
                kind: DiagnosticKind::MissingHandlerOps {
                    handler: program.defs[def].name.clone(),
                    ops: missing,
                    members: missing_members,
                },
            });
        }

        for (index, hop) in item.ops.iter().enumerate() {
            self.set_local(hop.self_local, handler_ty.clone());
            let op = match hop.op {
                Res::Def(op) if item.ops[..index].iter().any(|h| h.op == hop.op) => {
                    self.diags.push(Diagnostic {
                        span: Some(hop.name.span),
                        kind: DiagnosticKind::DuplicateHandlerOp {
                            op: program.defs[op].name.clone(),
                        },
                    });
                    None
                }
                Res::Def(op) => Some(op),
                // Name resolution has reported this.
                _ => None,
            };
            // If the implementation's `suspend` differs from the effect's declaration, E0415. The
            // parameters and body are checked as the implementation is written, so errors do not
            // cascade.
            let declared_suspend =
                op.map(|op| matches!(program.item(op), Some(Item::Op(o)) if o.sig.is_suspend));
            if let Some(declared) = declared_suspend
                && declared != hop.is_suspend
                && let Some(effect) = op.and_then(|op| program.defs[op].parent)
            {
                self.diags.push(Diagnostic {
                    span: Some(hop.name.span),
                    kind: DiagnosticKind::SuspendMismatch {
                        op: hop.name.name.clone(),
                        effect: program.defs[effect].name.clone(),
                        suspend: declared,
                    },
                });
            }
            let suspend = hop.is_suspend;
            // Effect type parameters are replaced with the types written in the handler head
            // for the effect that declares the operation (spec 8.1).
            let effect_args = op
                .and_then(|op| self.sigs.handler_effect_of(program, def, op))
                .map(|t| t.args.clone())
                .unwrap_or_default();
            // A capability-bound type of the effect in the result records the handler's
            // `self` (`[effect.handler]`).
            let mut rigid = Vec::new();
            let f = match op.and_then(|op| self.sigs.get(op)).map(|s| {
                let ty = substitute_bound(&s.scheme.ty, &effect_args);
                // A capability parameter is an unknown set in the implementation
                // (`[effect.handler]`), which ends with the body.
                let ty = match hop.body {
                    Some(body) => {
                        let rows: Vec<Row> = s
                            .scheme
                            .params
                            .iter()
                            .enumerate()
                            .map(|(i, p)| match &p.caps {
                                Some(tags) if p.name != "C(self)" => {
                                    let cap = CapElem {
                                        id: CapId::Rigid {
                                            binder: body,
                                            index: i as u32,
                                        },
                                        name: p
                                            .name
                                            .trim_start_matches("C(")
                                            .trim_end_matches(')')
                                            .into(),
                                        tag: tags.first().cloned().unwrap_or_else(|| Tag::new(def)),
                                    };
                                    rigid.push(cap.clone());
                                    Row::from_elems([RowElem::Cap(cap)])
                                }
                                _ => Row::from_elems([RowElem::Bound(i as u32)]),
                            })
                            .collect();
                        substitute(&ty, &[], &rows)
                    }
                    None => ty,
                };
                match s.scheme.params.last() {
                    Some(p) if p.name == "C(self)" => {
                        let recv = s.scheme.params.len() - 1;
                        let mut rows: Vec<Row> = (0..recv)
                            .map(|i| Row::from_elems([RowElem::Bound(i as u32)]))
                            .collect();
                        rows.push(Row::from_elems([RowElem::Cap(CapElem {
                            id: CapId::SelfOf(def),
                            name: SmolStr::new("self"),
                            tag: p
                                .caps
                                .as_ref()
                                .and_then(|t| t.first().cloned())
                                .unwrap_or_else(|| Tag::new(def)),
                        })]));
                        substitute(&ty, &[], &rows)
                    }
                    _ => ty,
                }
            }) {
                Some(Ty::Fn(f)) => Some(*f),
                _ => None,
            };
            let Some(f) = f else {
                // Operations whose type cannot be determined still have their expressions
                // inferred and types recorded.
                for p in &hop.params {
                    self.set_local(p.local, Ty::Error);
                }
                if let Some(body) = hop.body {
                    self.infer_expr(body);
                }
                continue;
            };
            // A suspend fn implementation's parameters are the operation's plus `resume`.
            let mut params = f.params.clone();
            if suspend {
                params.push(Ty::func(vec![f.ret.clone()], Ty::UNIT));
            }
            if hop.params.len() != params.len() {
                self.diags.push(Diagnostic {
                    span: Some(hop.name.span),
                    kind: DiagnosticKind::HandlerOpArgCount {
                        op: hop.name.name.clone(),
                        expected: params.len(),
                        found: hop.params.len(),
                    },
                });
            }
            for (i, p) in hop.params.iter().enumerate() {
                let ty = params.get(i).cloned().unwrap_or(Ty::Error);
                self.set_local(p.local, ty);
            }
            let Some(body) = hop.body else {
                continue;
            };
            // A suspend fn implementation's value is dropped; the operation's value is what is
            // passed to `resume`.
            let ret = if suspend { Ty::UNIT } else { f.ret.clone() };
            if !rigid.is_empty() {
                self.quantified_lambdas.insert(body, rigid);
            }
            self.ctx.scopes.enter(body);
            self.check_expr(body, &ret);
            self.ctx.scopes.exit();
            let resume_later = suspend
                && hop.params.len() == params.len()
                && hop
                    .params
                    .last()
                    .is_some_and(|resume| suspend::resume_escapes(self, body, resume.local));
            self.handler_bodies.push(HandlerBody {
                handler: def,
                index,
                body,
                fails: f.fails.clone(),
                resume_later,
                suspend,
                resume: (suspend && hop.params.len() == params.len())
                    .then(|| hop.params.last().map(|r| r.local))
                    .flatten(),
            });
        }
    }

    /// The type of opening `member` as the handler `handler` implements it: the member's type
    /// with the effect's type parameters replaced with the type arguments of the effect of
    /// `handler`'s outer handler (`[effect.handler]`).
    fn inner_member_ty(&self, handler: DefId, member: DefId) -> Option<crate::ty::FnTy> {
        let outer = self.program.defs[handler].parent?;
        let args = self
            .sigs
            .handler_effect_of(self.program, outer, member)
            .map(|t| t.args.clone())
            .unwrap_or_default();
        match self
            .sigs
            .get(member)
            .map(|s| substitute_bound(&s.scheme.ty, &args))
        {
            Some(Ty::Fn(f)) => Some(*f),
            _ => None,
        }
    }

    /// An inner handler implements the handler member of its name (`[effect.handler]`): it
    /// implements the effect the member declares, and its init takes the member's parameters with
    /// the same names and types (E0449).
    fn check_inner_handler(&mut self, def: DefId, member: DefId, item: &HandlerItem) {
        let program = self.program;
        let Some(f) = self.inner_member_ty(def, member) else {
            return;
        };
        let Some(Item::Member(m)) = program.item(member) else {
            return;
        };
        let cons = &self.ty_table.cons;
        let effect = program.defs[member]
            .parent
            .map(|e| program.defs[e].name.clone())
            .unwrap_or_default();
        let mismatch = |reason| DiagnosticKind::InnerHandlerMismatch {
            handler: program.defs[def].name.clone(),
            member: program.defs[member].name.clone(),
            effect: effect.clone(),
            reason: Box::new(reason),
        };
        // The inner handler implements the same effects as the member, in any order.
        let own = self.sigs.handler_effects(def);
        if let Ty::Cap(expected, _) = &f.ret
            && !own.is_empty()
            && !matches!(Ty::cap(own.iter().cloned(), Row::empty()), Ty::Cap(own, _) if own == *expected)
        {
            let show = |ts: &[Tag]| {
                let parts: Vec<String> = ts
                    .iter()
                    .map(|t| {
                        Row::of_tag(t.clone())
                            .display_effects(cons, &[])
                            .to_string()
                    })
                    .collect();
                SmolStr::new(parts.join(" + "))
            };
            self.diags.push(Diagnostic {
                span: program.defs[def].span,
                kind: mismatch(crate::diagnostic::InnerHandlerMismatch::Effect {
                    expected: show(expected),
                    found: show(own),
                }),
            });
        }
        let names = |ps: &[Param]| -> Vec<SmolStr> {
            ps.iter()
                .map(|p| program.locals[p.local].name.clone())
                .collect()
        };
        let show_params = |names: &[SmolStr], tys: &[Ty]| -> SmolStr {
            let parts: Vec<String> = names
                .iter()
                .zip(tys)
                .map(|(n, t)| format!("{n}: {}", t.display(cons)))
                .collect();
            SmolStr::new(format!("({})", parts.join(", ")))
        };
        let expected_names = names(&m.sig.params);
        let expected = show_params(&expected_names, &f.params);
        let found = match item.init {
            Some(_) => {
                let tys = self
                    .sigs
                    .init_params(def)
                    .map(|p| p.tys.clone())
                    .unwrap_or_default();
                let found_names = names(&item.init_params);
                let same = found_names == expected_names && tys == f.params;
                (!same).then(|| Some(show_params(&found_names, &tys)))
            }
            // Without an init, the handler takes no arguments.
            None => (!f.params.is_empty()).then_some(None),
        };
        if let Some(found) = found {
            self.diags.push(Diagnostic {
                span: program.defs[def].span,
                kind: mismatch(crate::diagnostic::InnerHandlerMismatch::Params {
                    params: expected,
                    found,
                }),
            });
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

    /// A handler's dependencies (the union of the R of init, release and the operation bodies):
    /// computed in an earlier group, or a set variable within this group. Empty for handlers not
    /// inferred (Async handlers).
    fn handler_deps(&self, handler: DefId) -> Row {
        if let Some(row) = self.handler_facts.uses.get(handler) {
            return row.clone();
        }
        match self.pending_handlers.iter().find(|(h, _, _)| *h == handler) {
            Some(&(_, v, _)) => Row::var(v),
            None => Row::empty(),
        }
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

    /// Whether the definition being inferred is a handler (where `@external` fns may be used,
    /// spec 12.2).
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
    /// Trait constraints and undetermined `Self` (E0326, E0333) are checked after numeric types,
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
                self.check_external(*def, e.span);
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
            // computed. A host effect has no operations; only the entry provides it
            // (`[effect.host]`).
            ExprKind::Use {
                effect: Res::Def(effect),
                ..
            } if program.is_host_effect(*effect) => {
                self.diags.push(Diagnostic {
                    span: Some(e.span),
                    kind: DiagnosticKind::UseHostEffect {
                        effect: program.defs[*effect].name.clone(),
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
            // omitting them is E0423.
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

    /// The arguments of init written on the items of a `with` (`[effect.with]`). Every item's
    /// arguments are checked, including those of a handler a later item overrides.
    ///
    /// An item that opens a handler member (`x = Files.Open(p)`, `x = fs.Open(p)`) takes the
    /// arguments of the member's init, and binds x to the capability of the effect the member
    /// declares (`[effect.with]`). Another bound item binds x to the capability of the effect it
    /// provides.
    ///
    /// The handler with type parameters that supplies an item is instantiated first
    /// (`body/items.rs`), so that the effects the item provides, the type of a name bound to it
    /// and the parameters of its init have the same type variables.
    fn infer_with_args(&mut self, expr: ExprId, items: &[WithItem]) {
        let program = self.program;
        let insts: Vec<items::ItemInst> = items
            .iter()
            .map(|item| {
                if crate::layer::is_member_item(program, item) {
                    return items::ItemInst::default();
                }
                // A label left of `by` takes no type arguments (E0304).
                if item.by.is_some() && !item.ty_args.is_empty() {
                    self.instantiate_item(item.res, &item.ty_args, item.span, false);
                }
                let (target, _) = item.supplier();
                let span = item.by.map_or(item.span, |(_, s)| s);
                let written = match (item.by, target) {
                    // The type arguments of an effect right of `by` are read by `by_tag`.
                    (Some(_), Res::Def(d)) if program.defs[d].kind == DefKind::Effect => &[][..],
                    _ => item.supplier_ty_args(),
                };
                let mut inst = self.instantiate_item(target, written, span, false);
                if item.by.is_some() {
                    self.unify_with_label(item.res, &inst);
                }
                self.freeze(&mut inst);
                inst
            })
            .collect();
        self.item_insts.insert(expr, insts.clone());
        for (i, inst) in insts.iter().enumerate() {
            for arg in &inst.args {
                if let Ty::Set(_, row) = arg
                    && let [RowElem::Var(v)] = row.elems()
                {
                    self.item_set_vars.insert(*v, (expr, i as u32));
                }
            }
        }
        let insts_now = self.item_insts(expr, items);
        let entries =
            crate::layer::with_entries(program, self.sigs, self.ty_table, items, &insts_now);
        let mut opened = Vec::new();
        let args = items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                if let Some((f, member)) = self.infer_member_item(item) {
                    let ret = f.ret.clone();
                    let args = match &item.args {
                        Some(args) => {
                            let labels = self.sigs.get(member).map(|s| s.labels.clone());
                            self.check_args(
                                Some(&program.defs[member].name),
                                labels.as_deref(),
                                &f.params,
                                &args.args,
                                args.span,
                            )
                        }
                        // A member whose init takes parameters needs them, as a handler does
                        // (E0434, `[effect.with]`).
                        None => {
                            if !f.params.is_empty() {
                                let labels = self.sigs.get(member).map(|s| s.labels.clone());
                                self.diags.push(Diagnostic {
                                    span: Some(item.span),
                                    kind: DiagnosticKind::MissingMemberArgs {
                                        member: program.defs[member].name.clone(),
                                        effect: item
                                            .label
                                            .map(|l| program.defs[l].name.clone())
                                            .unwrap_or_else(|| self.member_effect_name(member)),
                                        params: labels
                                            .unwrap_or_default()
                                            .into_iter()
                                            .flatten()
                                            .collect(),
                                    },
                                });
                            }
                            Vec::new()
                        }
                    };
                    // The capability of the item itself (`[effect.with]`).
                    if let Some(local) = item.bind {
                        let ty = match self.ctx.resolve(&ret) {
                            Ty::Cap(tags, _) if !tags.is_empty() => {
                                let cap = self.item_cap(
                                    expr,
                                    i,
                                    program.locals[local].name.clone(),
                                    tags[0].clone(),
                                );
                                Ty::Cap(tags, Row::from_elems([cap]))
                            }
                            ty => ty,
                        };
                        self.set_local(local, ty);
                        self.member_bound.insert(local);
                    }
                    opened.push((i, f));
                    return args;
                }
                if let Res::Def(def) = item.res
                    && item.bind.is_some()
                    && matches!(program.item(def), Some(Item::Layer(_)))
                {
                    self.diags.push(Diagnostic {
                        span: Some(item.span),
                        kind: DiagnosticKind::BoundLayer {
                            layer: program.defs[def].name.clone(),
                        },
                    });
                }
                // The capability of every effect the item provides: all of them for a handler
                // of several effects (`[effect.with]`).
                if let Some(local) = item.bind {
                    let tags: Vec<Tag> = entries
                        .iter()
                        .filter(|e| e.origin == i && !e.from_layer)
                        .map(|e| e.effect.clone())
                        .collect();
                    let ty = if tags.is_empty() {
                        Ty::Error
                    } else {
                        let cap = self.item_cap(
                            expr,
                            i,
                            program.locals[local].name.clone(),
                            tags[0].clone(),
                        );
                        Ty::cap(tags, Row::from_elems([cap]))
                    };
                    self.set_local(local, ty);
                }
                let ty_args = &insts[i].args;
                match item.by {
                    // The label left of `by` takes no arguments.
                    Some((by, by_span)) => {
                        self.infer_supply_args(item.res, item.args.as_ref(), item.span, &[]);
                        self.infer_supply_args(by, item.by_init_args.as_ref(), by_span, ty_args)
                    }
                    None => {
                        self.infer_supply_args(item.res, item.args.as_ref(), item.span, ty_args)
                    }
                }
            })
            .collect();
        self.with_args.insert(expr, args);
        // The arguments of init may have determined the type arguments; only then does the item
        // take part in overriding (`[effect.with]`).
        for (i, inst) in insts.iter().enumerate() {
            if !inst.args.is_empty() && !inst.determined && self.inst_known(inst) {
                let mut inst = inst.clone();
                inst.determined = true;
                self.freeze(&mut inst);
                if let Some(v) = self.item_insts.get_mut(expr) {
                    v[i] = inst;
                }
            }
        }
        let insts_now = self.item_insts(expr, items);
        let entries =
            crate::layer::with_entries(program, self.sigs, self.ty_table, items, &insts_now);
        self.check_item_overrides(expr, &entries);
        if !opened.is_empty() {
            self.with_members.insert(expr, opened);
        }
    }

    /// The handler member an item of `with` opens (`Files.Open(p)` or `fs.Open(p)`).
    pub(super) fn member_item(&self, item: &WithItem) -> Option<DefId> {
        match (item.res, &item.cap) {
            (Res::Def(def), _) if self.program.defs[def].kind == DefKind::Member => Some(def),
            (_, Some((cap, _))) => self.member_opens.get(*cap).copied(),
            _ => None,
        }
    }

    /// For an item of `with` that opens a handler member, the type of opening it
    /// (`[type.rules]`): the parameters of the init, the capability opened, the member's E, and
    /// the R that names the effect (`Files`) or the capability (`fs`). An item without `x =` is
    /// E0447.
    fn infer_member_item(&mut self, item: &WithItem) -> Option<(crate::ty::FnTy, DefId)> {
        let program = self.program;
        let (ty, member) = match (item.res, &item.cap) {
            (Res::Def(member), _) if program.defs[member].kind == DefKind::Member => {
                let ty = self.instantiate_def(member, item.span).0;
                let ty = match item.label {
                    Some(label) => through_label(ty, label),
                    None => ty,
                };
                (ty, member)
            }
            (_, Some((cap, name))) => {
                let cap_ty = self.infer_expr(*cap);
                let tags = match self.ctx.shallow_resolve(&cap_ty) {
                    Ty::Cap(tags, _) => tags,
                    Ty::Error => return None,
                    ty => {
                        let ty = self.ctx.resolve(&ty);
                        self.diags.push(Diagnostic {
                            span: Some(name.span),
                            kind: DiagnosticKind::NoFieldOrFunction {
                                ty,
                                name: name.name.clone(),
                            },
                        });
                        return None;
                    }
                };
                let found = tags.iter().find_map(|t| {
                    program
                        .effect_member(t.def, &name.name)
                        .map(|m| (t.clone(), m))
                });
                let Some((tag, member)) = found else {
                    self.diags.push(Diagnostic {
                        span: Some(name.span),
                        kind: DiagnosticKind::UnknownOperation {
                            effect: cap_effects_name(program, &tags),
                            op: name.name.clone(),
                        },
                    });
                    return None;
                };
                self.member_opens.insert(*cap, member);
                (self.cap_member_ty(*cap, &tag, member), member)
            }
            _ => return None,
        };
        if item.bind.is_none() {
            self.diags.push(Diagnostic {
                span: Some(item.span),
                kind: DiagnosticKind::UnboundMemberItem {
                    member: program.defs[member].name.clone(),
                    effect: self.member_effect_name(member),
                },
            });
        }
        match ty {
            Ty::Fn(f) => Some((*f, member)),
            _ => None,
        }
    }

    /// Checks the arguments written after `target` (`WorkerKv(binding: "KV")`) against the
    /// parameters of its init, as the arguments of a call (`[effect.handler]`, spec 6.2), and
    /// returns them reordered by parameter. A handler whose init takes parameters needs them
    /// (E0434); anything else takes none (E0435). `span` is where `target` is written.
    ///
    /// For a handler with type parameters, the types of the parameters are instantiated with
    /// `ty_args`, the type arguments of the item (`[effect.with]`).
    fn infer_supply_args(
        &mut self,
        target: Res,
        args: Option<&SupplyArgs>,
        span: Span,
        ty_args: &[Ty],
    ) -> Vec<Option<ExprId>> {
        let program = self.program;
        let sigs = self.sigs;
        let handler = match target {
            Res::Def(def) if matches!(program.item(def), Some(Item::Handler(_))) => Some(def),
            _ => None,
        };
        let instantiated = handler.and_then(|h| sigs.init_params(h)).map(|p| {
            let mut p = p.clone();
            if !ty_args.is_empty() {
                p.tys = p.tys.iter().map(|t| substitute_bound(t, ty_args)).collect();
            }
            p
        });
        let params = handler.zip(instantiated.as_ref());
        match (params, args) {
            (None, None) => Vec::new(),
            (Some((h, params)), None) => {
                self.diags.push(Diagnostic {
                    span: Some(span),
                    kind: DiagnosticKind::MissingInitArgs {
                        handler: program.defs[h].name.clone(),
                        params: params.labels.iter().flatten().cloned().collect(),
                    },
                });
                Vec::new()
            }
            (None, Some(args)) => {
                if let Res::Def(def) = target {
                    let kind = match program.defs[def].kind {
                        DefKind::Effect if crate::signature::is_label(program, def) => "label",
                        kind => kind.describe(),
                    };
                    self.diags.push(Diagnostic {
                        span: Some(args.span),
                        kind: DiagnosticKind::UnexpectedInitArgs {
                            name: program.defs[def].name.clone(),
                            kind,
                        },
                    });
                }
                for arg in &args.args {
                    self.infer_expr(arg.value);
                }
                Vec::new()
            }
            (Some((h, params)), Some(args)) => {
                // The item decides the capability sets of the parameters.
                let rows: Vec<Row> = (0..params.caps)
                    .map(|_| Row::var(self.ctx.new_row_var()))
                    .collect();
                let tys: Vec<Ty> = params
                    .tys
                    .iter()
                    .map(|t| substitute(t, &[], &rows))
                    .collect();
                self.check_args(
                    Some(&program.defs[h].name),
                    Some(&params.labels),
                    &tys,
                    &args.args,
                    args.span,
                )
            }
        }
    }

    /// A handler value's constructor can only be used inside that handler's init (E0418, spec
    /// 8.4), since constructing it elsewhere would skip init and release.
    fn check_handler_ctor(&mut self, def: DefId, span: Span) {
        let program = self.program;
        let Some(Item::Ctor(ctor)) = program.item(def) else {
            return;
        };
        if program.defs[ctor.owner].kind != DefKind::Handler || self.in_init == Some(ctor.owner) {
            return;
        }
        self.diags.push(Diagnostic {
            span: Some(span),
            kind: DiagnosticKind::HandlerCtorOutsideInit {
                handler: program.defs[ctor.owner].name.clone(),
            },
        });
    }

    /// A value of a capability-bound type of E can be constructed only inside a handler of E
    /// (E0458, `[type.rules]`). Returns the capability the value records: the handler's `self`.
    fn check_bound_ctor(&mut self, def: DefId, span: Span) -> Option<CapElem> {
        let program = self.program;
        let owner = match program.item(def) {
            Some(Item::Ctor(ctor)) => ctor.owner,
            _ => return None,
        };
        let effect = self.ty_table.bound_effect(owner)?;
        let handler = self.current.filter(|_| self.in_handler());
        let implements = handler.and_then(|h| {
            self.sigs
                .handler_effects(h)
                .iter()
                .find(|t| t.def == effect)
                .map(|t| (h, t.clone()))
        });
        match implements {
            Some((h, tag)) => Some(CapElem {
                id: CapId::SelfOf(h),
                name: SmolStr::new("self"),
                tag,
            }),
            None => {
                self.diags.push(Diagnostic {
                    span: Some(span),
                    kind: DiagnosticKind::BoundTypeOutsideHandler {
                        ty: program.defs[owner].name.clone(),
                        effect: program.defs[effect].name.clone(),
                    },
                });
                None
            }
        }
    }

    /// `self` in an operation body or in release, passed to a parameter of type `use E`, is the
    /// capability of the handler (`[effect.handler]`): its type is recorded as such, and lowering
    /// makes the handler's value from the state.
    fn self_as_capability(&mut self, arg: ExprId, actual: Ty) -> Ty {
        let program = self.program;
        let ExprKind::Path(Res::Local(l)) = program.exprs[arg].kind else {
            return actual;
        };
        let Some(handler) = self.current.filter(|_| self.in_handler()) else {
            return actual;
        };
        if program.locals[l].kind != LocalKind::SelfParam || self.in_init.is_some() {
            return actual;
        }
        let tags = self.sigs.handler_effects(handler).to_vec();
        let Some(first) = tags.first().cloned() else {
            return actual;
        };
        let cap = Ty::cap(
            tags,
            Row::from_elems([RowElem::Cap(CapElem {
                id: CapId::SelfOf(handler),
                name: SmolStr::new("self"),
                tag: first,
            })]),
        );
        self.exprs.insert(arg, cap.clone());
        self.self_caps.insert(arg);
        cap
    }

    /// `ty`, the type `def` names, with its hidden capability sets replaced by fresh set variables.
    fn fresh_hidden_caps(&mut self, def: DefId, ty: &Ty) -> Ty {
        let (Ty::Named(con, args), Some(caps)) = (ty, self.ty_table.decl_caps(def)) else {
            return ty.clone();
        };
        let visible = caps.visible;
        let args = args
            .iter()
            .enumerate()
            .map(|(i, a)| {
                if i >= visible {
                    Ty::Set(SetKind::Caps, Row::var(self.ctx.new_row_var()))
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>();
        Ty::named(*con, args)
    }

    /// Puts `cap` in the set of the capability-bound type that the constructor type `ty` makes.
    fn record_maker(&mut self, ty: &Ty, cap: CapElem) {
        let made = match ty {
            Ty::Fn(f) => &f.ret,
            ty => ty,
        };
        let Ty::Named(con, args) = made else {
            return;
        };
        let def = self.ty_table.def(*con);
        let Some(i) = self.ty_table.decl_caps(def).map(|c| c.visible) else {
            return;
        };
        if let Some(Ty::Set(SetKind::Caps, row)) = args.get(i) {
            self.ctx.add_subset_of(
                SetKind::Caps,
                Row::from_elems([RowElem::Cap(cap)]),
                row.clone(),
                Origin::Lambda,
            );
        }
    }

    /// An `@external` fn with `uses` omitted or with host effects in it, and an
    /// `@external suspend fn`, can only be used inside handler operation bodies (`[ext.call]`).
    /// Checked on every reference, not only calls, so they cannot be smuggled out as function
    /// values. Only `uses {}` lifts the restriction; any other effect in the clause is E0445.
    fn check_external(&mut self, def: DefId, span: Span) {
        let Some(Item::Fn(item)) = self.program.item(def) else {
            return;
        };
        if !item.is_external || self.in_handler() {
            return;
        }
        if item.sig.is_suspend || item.sig.uses.as_ref().is_none_or(|u| !u.items.is_empty()) {
            self.diags.push(Diagnostic {
                span: Some(span),
                kind: DiagnosticKind::ExternalOutsideHandler {
                    name: self.program.defs[def].name.clone(),
                    suspend: item.sig.is_suspend,
                    host: item.sig.uses.as_ref().is_some_and(|u| !u.items.is_empty()),
                },
            });
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

    /// A local fn (`[fn.local]`), checked at its position in the block. Its name is bound to its
    /// function type before the body is checked, so a call to itself in the body has the type
    /// being inferred and is not generalized (`[type.rules]`).
    ///
    /// The signature follows `[type.inference]` as for a top-level fn: parameter types are
    /// written (E0316), an omitted return type is inferred with E, an omitted `fails` with a
    /// return type means no errors, and an omitted `uses` is inferred. The E and R of function
    /// types in the parameters whose clauses are omitted are set variables shared by every use,
    /// and those the body can call are part of the declared E and R. The E and R of the body are
    /// constrained to the type's E and R when E and R are computed (`body/errors.rs`,
    /// `body/effects.rs`); the declaration itself has none.
    fn infer_local_fn(&mut self, f: &LocalFn) {
        let program = self.program;
        let outer = self.binder.replace(f.body);
        let mut hidden_fails = Row::empty();
        let mut hidden_uses = Row::empty();
        let mut params = Vec::new();
        for p in &f.sig.params {
            let ty = match p.ty {
                Some(ty) => {
                    let mut scope = Scope {
                        params: &self.type_params,
                        kinds: &self.param_kinds,
                        omitted: Omitted::Local {
                            ctx: &mut self.ctx,
                            positive: &mut hidden_fails,
                            positive_uses: &mut hidden_uses,
                        },
                        sets: true,
                        self_ty: self.self_ty.as_ref(),
                        result_caps: None,
                        quant: None,
                    };
                    self.ty_table
                        .lower_in(program, &mut scope, ty, true, &mut self.diags)
                }
                None => {
                    let local = &program.locals[p.local];
                    self.diags.push(Diagnostic {
                        span: Some(local.span),
                        kind: DiagnosticKind::MissingLocalFnParamType {
                            param: local.name.clone(),
                        },
                    });
                    Ty::Error
                }
            };
            self.set_local(p.local, ty.clone());
            params.push(ty);
        }
        let ret = match f.sig.ret {
            Some(ty) => self.lower_type(ty),
            None => self.ctx.new_var(),
        };
        let scope = Scope {
            params: &self.type_params,
            kinds: &self.param_kinds,
            omitted: Omitted::Empty,
            sets: true,
            self_ty: self.self_ty.as_ref(),
            result_caps: None,
            quant: None,
        };
        let fails_declared = f.sig.ret.is_some();
        let fails = if fails_declared {
            let written = match &f.sig.fails {
                Some(set) => self
                    .ty_table
                    .lower_error_set(program, &scope, set, &mut self.diags),
                None => Row::empty(),
            };
            written.union(&hidden_fails)
        } else {
            Row::var(self.ctx.new_row_var())
        };
        let uses = match &f.sig.uses {
            Some(set) => self
                .ty_table
                .lower_effect_set(program, &scope, set, &mut self.diags)
                .union(&hidden_uses),
            None => Row::var(self.ctx.new_row_var()),
        };
        let ty = Ty::func_sets(params, ret.clone(), fails.clone(), uses.clone());
        // The name belongs to the enclosing body; the parameters and the body to the local fn.
        let inner = std::mem::replace(&mut self.binder, outer);
        self.set_local(f.local, ty);
        self.binder = inner;
        self.local_fns.insert(
            f.local,
            LocalFnInfo {
                labels: param_labels(program, &f.sig.params),
                body: f.body,
                fails,
                uses,
                fails_declared,
                fails_written: f.sig.fails.is_some(),
                uses_written: f.sig.uses.is_some(),
            },
        );
        self.name_providers.push(None);
        self.ctx.scopes.enter(f.body);
        self.check_expr(f.body, &ret);
        self.ctx.scopes.exit();
        self.name_providers.pop();
        self.binder = outer;
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

    /// Replaces the declared type's type parameters with fresh type variables.
    ///
    /// Definitions in the same group without a declared type use their monomorphic type as is
    /// (not instantiated).
    fn infer_def_ref(&mut self, def: DefId, span: Span) -> Ty {
        self.instantiate_def(def, span).0
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

    /// Gives the reference's function type `instance` its own E and R where the referenced
    /// definition's are inferred (contain set variables). Their solutions can name that
    /// definition's type parameters and invisible sets, so they are placed with the reference's
    /// arguments once solved ([`InstanceSet`]) instead of being shared.
    ///
    /// `ty` is the referenced definition's monomorphic type, resolved.
    fn instance_sets(&mut self, ty: &Ty, instance: Ty, args: &[Ty], rows: &[Row]) -> Ty {
        let (Ty::Fn(def_fn), Ty::Fn(f)) = (ty, &instance) else {
            return instance;
        };
        let place = |inferred: &Row, kind: SetKind, this: &mut Self| -> Option<Row> {
            inferred.vars().next()?;
            let v = this.ctx.new_row_var();
            this.instance_sets.push(InstanceSet {
                inferred: inferred.clone(),
                instance: v,
                kind,
                args: args.to_vec(),
                rows: rows.to_vec(),
            });
            Some(Row::var(v))
        };
        let fails = place(&def_fn.fails, SetKind::Errors, self);
        let uses = place(&def_fn.uses, SetKind::Effects, self);
        if fails.is_none() && uses.is_none() {
            return instance;
        }
        Ty::func_sets(
            f.params.clone(),
            f.ret.clone(),
            fails.unwrap_or_else(|| f.fails.clone()),
            uses.unwrap_or_else(|| f.uses.clone()),
        )
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

    /// Remembers that `ty` must satisfy the Prelude's trait `name` at `span`: the operand of
    /// `==` (Eq) or a comparison (Ord, Float aside), or an interpolated value (Show) (spec 10.3).
    fn require_prelude_trait(&mut self, ty: &Ty, name: &str, span: Span) {
        let Some(trait_def) = self.program.prelude_def(name) else {
            return;
        };
        self.trait_obligations.push(TraitObligation {
            ty: ty.clone(),
            trait_def,
            span,
            owner: self.current,
            float_ok: name == "Ord",
        });
    }

    /// Remembers the `Self` of a trait function whose parameters do not mention `Self`
    /// (`Parse.parse`): only the result type picks the impl, so it must be known once the group is
    /// inferred (E0333, spec 10.3).
    fn require_known_self(&mut self, def: DefId, scheme: &crate::Scheme, tys: &[Ty], span: Span) {
        let program = self.program;
        if program.defs[def].kind != DefKind::TraitFn {
            return;
        }
        let (Some(self_ty), Some(param), Ty::Fn(f)) =
            (tys.first(), scheme.params.first(), &scheme.ty)
        else {
            return;
        };
        let Some(&trait_def) = param.traits.first() else {
            return;
        };
        if f.params.iter().any(|p| mentions_bound(p, 0)) {
            return;
        }
        self.self_obligations.push(SelfObligation {
            ty: self_ty.clone(),
            trait_def,
            span,
        });
    }

    /// Reports the `Self` of trait functions left undetermined (E0333, spec 10.3). No type is
    /// chosen by default.
    fn check_known_selves(&mut self) {
        let program = self.program;
        for o in std::mem::take(&mut self.self_obligations) {
            if let Ty::Var(_) = self.ctx.resolve(&o.ty) {
                self.diags.push(Diagnostic {
                    span: Some(o.span),
                    kind: DiagnosticKind::AmbiguousSelf {
                        trait_name: program.defs[o.trait_def].name.clone(),
                    },
                });
            }
        }
    }

    /// Checks the trait constraints remembered at references (E0326, spec 10.3). A type
    /// parameter of the definition with the reference satisfies what its own constraints imply,
    /// supertraits included.
    fn check_traits(&mut self) {
        let program = self.program;
        for o in std::mem::take(&mut self.trait_obligations) {
            let ty = self.ctx.resolve(&o.ty);
            if o.float_ok && ty == Ty::FLOAT {
                continue;
            }
            let env = o.owner.map(|d| self.def_params(d)).unwrap_or_default();
            if let Err(e) = self
                .traits
                .satisfies(program, self.ty_table, &ty, o.trait_def, &env)
            {
                // Unsolved sets (`fails ?e1`) of a function type say nothing here.
                let e = crate::traits::Unsatisfied {
                    ty: without_set_vars(&e.ty),
                    ..e
                };
                self.diags.push(Diagnostic {
                    span: Some(o.span),
                    kind: crate::traits::not_implemented(program, self.ty_table, &e, &env),
                });
            }
        }
    }

    /// Reorders the arguments by parameter position, then checks types (spec 6.2). `expr` is
    /// where the reordered arguments are recorded (the call or the pipe expression).
    fn infer_call(&mut self, expr: ExprId, callee: ExprId, args: &[Arg], span: Span) -> Ty {
        let program = self.program;
        let sigs = self.sigs;

        // A local fn called by its name takes named arguments (`[fn.args]`). Its labels are
        // recorded at its declaration, which comes before every call to it.
        let local_labels = match &program.exprs[callee].kind {
            ExprKind::Path(Res::Local(local)) => self
                .local_fns
                .get(*local)
                .map(|info| (&program.locals[*local].name, info.labels.clone())),
            _ => None,
        };
        let (callee_name, labels) = match (&program.exprs[callee].kind, &local_labels) {
            (ExprKind::Path(Res::Def(def)), _) => (
                Some(&program.defs[*def].name),
                sigs.get(*def).map(|sig| sig.labels.as_slice()),
            ),
            (_, Some((name, labels))) => (Some(*name), Some(labels.as_slice())),
            _ => (None, None),
        };
        // A handler member is opened by calling it (`Files.Open(p)`, `fs.Open(p)`).
        self.member_callees.insert(callee);
        // `x.f(a)` may be a method call (`[type.rules]`).
        let callee_ty = match &program.exprs[callee].kind {
            ExprKind::Field { base, name } => {
                let base_ty = self.infer_expr(*base);
                match self.infer_dot(callee, *base, &base_ty, name, true) {
                    methods::Dot::Method(def) => {
                        return self.infer_method_call(
                            expr, callee, *base, &base_ty, def, name, args, span,
                        );
                    }
                    methods::Dot::Value(ty) => {
                        self.exprs.insert(callee, ty.clone());
                        ty
                    }
                }
            }
            _ => self.infer_expr(callee),
        };
        // Capability operations (`log.info(...)`) use the declared operation's parameter names as
        // labels.
        let (callee_name, labels) = match (callee_name, self.ops.get(callee)) {
            (None, Some(&op)) => (
                Some(&program.defs[op].name),
                sigs.get(op).map(|sig| sig.labels.as_slice()),
            ),
            _ => (callee_name, labels),
        };

        let f: FnTy = match self.ctx.shallow_resolve(&callee_ty) {
            // A function of a quantified type takes the sets of its arguments, as a capability
            // parameter does (`[type.rules]`): each call puts a fresh set variable in their place.
            // The callee's type is recorded opened, so that its R names them.
            Ty::Fn(f) if f.quantifies() => {
                let opened = Ty::Fn(f).open_quant(&mut |_, _| Row::var(self.ctx.new_row_var()));
                self.exprs.insert(callee, opened.clone());
                match opened {
                    Ty::Fn(f) => *f,
                    _ => unreachable!("a function type stays one"),
                }
            }
            Ty::Fn(f) => *f,
            Ty::Error => {
                for arg in args {
                    self.infer_expr(arg.value);
                }
                return Ty::Error;
            }
            _ => {
                let params: Vec<Ty> = (0..args.len()).map(|_| self.ctx.new_var()).collect();
                let ret = self.ctx.new_var();
                let fails = Row::var(self.ctx.new_row_var());
                let uses = Row::var(self.ctx.new_row_var());
                let expected =
                    Ty::func_sets(params.clone(), ret.clone(), fails.clone(), uses.clone());
                let callee_span = program.exprs[callee].span;
                self.coerce_at(callee_span, &expected, &callee_ty);
                FnTy {
                    params,
                    ret,
                    fails,
                    uses,
                }
            }
        };

        self.callee_uses = Some(f.uses.clone());
        let slots = self.check_args(callee_name, labels, &f.params, args, span);
        self.calls.insert(expr, slots);

        f.ret
    }

    /// Assigns `args` to the parameters `params` ([`Self::assign_args`]), reports a wrong number
    /// of them at `span` (E0305), checks each argument against its parameter's type and infers
    /// the extra ones (spec 6.2). Returns the arguments in parameter order. Shared by calls and
    /// the arguments of init (`[effect.handler]`).
    fn check_args(
        &mut self,
        callee_name: Option<&SmolStr>,
        labels: Option<&[Option<SmolStr>]>,
        params: &[Ty],
        args: &[Arg],
        span: Span,
    ) -> Vec<Option<ExprId>> {
        let assigned = self.assign_args(
            callee_name,
            labels,
            params.len(),
            args.iter().map(|arg| (arg.label.as_ref(), arg.value)),
        );
        if !assigned.reported && (assigned.too_many || assigned.has_missing()) {
            self.diags.push(Diagnostic {
                span: Some(span),
                kind: DiagnosticKind::WrongArgCount {
                    callee: callee_name.cloned(),
                    expected: params.len(),
                    found: args.len(),
                },
            });
        }
        let own = self.callee_uses.take();
        for (slot, param_ty) in assigned.slots.iter().zip(params) {
            if let Some(arg) = slot {
                // A function whose parameter's R writes out an effect that its own R does not
                // have provides that effect by name to the argument (`[effect.escape]`).
                if let Some(own) = &own
                    && matches!(self.program.exprs[*arg].kind, ExprKind::Lambda { .. })
                    && let Ty::Fn(f) = self.ctx.shallow_resolve(param_ty)
                {
                    let provided: Vec<Tag> = f
                        .uses
                        .effect_tags()
                        .filter(|t| !own.effect_tags().any(|o| o.def == t.def))
                        .cloned()
                        .collect();
                    if !provided.is_empty() {
                        self.provided_to_lambda = Some(provided);
                    }
                }
                if let Ty::Cap(wanted, _) = self.ctx.shallow_resolve(param_ty) {
                    self.check_cap_arg(*arg, &wanted, param_ty);
                } else {
                    self.check_expr(*arg, param_ty);
                }
            }
        }
        for extra in assigned.extra {
            self.infer_expr(extra);
        }
        assigned.slots
    }

    /// An argument passed to a parameter of type `use F1 + ... + Fm` (`[effect.use]`). A
    /// capability of `use E1 + ... + En` can be passed when every Fi is one of the Ej; when the
    /// parameter has fewer effects, the argument is recorded in `narrowings`, and only the
    /// operations of the Fi can be called through the parameter. A capability without some Fi is
    /// E0452. Anything else is checked against the parameter type as usual.
    fn check_cap_arg(&mut self, arg: ExprId, wanted: &[Tag], param_ty: &Ty) {
        let actual = self.infer_expr(arg);
        let actual = self.self_as_capability(arg, actual);
        let span = self.program.exprs[arg].span;
        let Ty::Cap(have, have_caps) = self.ctx.shallow_resolve(&actual) else {
            self.coerce_at(span, param_ty, &actual);
            return;
        };
        let wanted_caps = match self.ctx.shallow_resolve(param_ty) {
            Ty::Cap(_, caps) => caps,
            _ => Row::empty(),
        };
        let mut missing = Vec::new();
        let mut matched = Vec::with_capacity(wanted.len());
        for w in wanted {
            match have
                .iter()
                .find(|h| h.def == w.def && h.args.len() == w.args.len())
            {
                Some(h) => matched.push((w.clone(), h.clone())),
                None => missing.push(self.program.defs[w.def].name.clone()),
            }
        }
        if !missing.is_empty() {
            let cons = &self.ty_table.cons;
            let expected = SmolStr::new(self.ctx.resolve(param_ty).display(cons).to_string());
            let found = SmolStr::new(self.ctx.resolve(&actual).display(cons).to_string());
            self.diags.push(Diagnostic {
                span: Some(span),
                kind: DiagnosticKind::MissingCapabilityEffects {
                    expected,
                    found,
                    missing,
                },
            });
            return;
        }
        // The type arguments of each effect kept must match (spec 8.1), and the capabilities
        // the argument may be go into the set of the parameter (`[type.rules]`).
        for (w, h) in &matched {
            self.coerce_at(
                span,
                &Ty::cap([w.clone()], wanted_caps.clone()),
                &Ty::cap([h.clone()], have_caps.clone()),
            );
        }
        if have.len() > wanted.len() {
            self.narrowings.insert(arg, wanted.to_vec());
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

    /// If the expected type is a function type with the same number of parameters, its parameter
    /// types are passed to the lambda's parameters first. The body's E and R do not escape; they
    /// are enclosed in the function type's E and R (fresh set variables) (spec 9.4). The
    /// constraints putting the body's E and R into those set variables are added when the body's
    /// E and R are computed (`body/errors.rs`, `body/effects.rs`).
    fn infer_lambda(
        &mut self,
        expr: ExprId,
        params: &[Param],
        body: ExprId,
        span: Span,
        expected: Option<&Ty>,
    ) -> Ty {
        let expected_fn = match expected.map(|ty| self.ctx.shallow_resolve(ty)) {
            Some(Ty::Fn(f)) if f.params.len() == params.len() => Some(*f),
            _ => None,
        };
        // Checked against a function type that quantifies capability sets, the closure receives
        // an unknown set for each (`[type.rules]`): a capability of its own scope.
        let quantified = expected_fn.as_ref().is_some_and(|f| f.quantifies());
        let mut rigid = Vec::new();
        let expected_fn = match expected_fn {
            Some(f) if quantified => {
                // Named after the parameter whose type has it (`s` of `|s|`).
                let names: Vec<Option<SmolStr>> = f
                    .params
                    .iter()
                    .zip(params)
                    .map(|(ty, p)| {
                        let name = &self.program.locals[p.local].name;
                        let mut has = false;
                        ty.visit(&mut |t| {
                            if let Ty::Cap(_, row) | Ty::Set(SetKind::Caps, row) = t {
                                has |= row.caps().any(|c| matches!(c.id, CapId::Quant(_)));
                            }
                        });
                        (has && name != "_").then(|| name.clone())
                    })
                    .collect();
                let opened = Ty::Fn(Box::new(f.clone())).open_quant(&mut |i, c| {
                    let name = f
                        .params
                        .iter()
                        .position(|ty| {
                            let mut has = false;
                            ty.visit(&mut |t| {
                                if let Ty::Cap(_, row) | Ty::Set(SetKind::Caps, row) = t {
                                    has |= row
                                        .caps()
                                        .any(|c| matches!(c.id, CapId::Quant(j) if j == i));
                                }
                            });
                            has
                        })
                        .and_then(|k| names[k].clone())
                        .unwrap_or_else(|| c.name.clone());
                    let cap = CapElem {
                        id: CapId::Rigid {
                            binder: expr,
                            index: i,
                        },
                        name,
                        tag: c.tag.clone(),
                    };
                    rigid.push(cap.clone());
                    Row::from_elems([RowElem::Cap(cap)])
                });
                match opened {
                    Ty::Fn(f) => Some(*f),
                    _ => None,
                }
            }
            f => f,
        };
        if quantified {
            self.quantified_lambdas.insert(expr, rigid);
        }
        let outer = self.binder.replace(expr);
        self.name_providers.push(None);
        let provided = self.provided_to_lambda.take();
        let by_name = provided.is_some();
        if let Some(provided) = provided {
            self.name_providers.push(Some((expr, provided)));
        }
        self.ctx.scopes.enter(expr);

        let mut param_tys = Vec::new();
        for (i, param) in params.iter().enumerate() {
            let from_context = expected_fn.as_ref().map(|f| f.params[i].clone());
            let ty = match (param.ty, from_context) {
                (Some(annotation), from_context) => {
                    let ty = self.lower_type(annotation);
                    if let Some(from_context) = from_context {
                        let span = self.program.locals[param.local].span;
                        self.coerce_at(span, &from_context, &ty);
                    }
                    ty
                }
                (None, Some(from_context)) => from_context,
                (None, None) => self.ctx.new_var(),
            };
            self.set_local(param.local, ty.clone());
            param_tys.push(ty);
        }

        let ret_ty = match &expected_fn {
            Some(f) => {
                self.check_expr(body, &f.ret);
                f.ret.clone()
            }
            None => self.infer_expr(body),
        };
        self.ctx.scopes.exit();
        if by_name {
            self.name_providers.pop();
        }
        self.name_providers.pop();
        self.binder = outer;

        let row = self.ctx.new_row_var();
        let uses = self.ctx.new_row_var();
        self.lambda_rows.insert(expr, (row, uses));
        let ty = Ty::func_sets(param_tys, ret_ty, Row::var(row), Row::var(uses));
        let ty = if quantified {
            self.requantify(&ty, expr)
        } else {
            ty
        };
        let origin = Origin::Coerce {
            span: Some(span),
            def: self.current,
        };
        match (expected, &expected_fn) {
            // The lambda's E and R are contained in the expected type's E and R.
            (_, Some(f)) => {
                self.ctx
                    .add_subset(Row::var(row), f.fails.clone(), origin.clone());
                self.ctx
                    .add_subset_of(SetKind::Effects, Row::var(uses), f.uses.clone(), origin);
            }
            // If the expected type was not a function type (or had a different parameter count),
            // check here to report the error.
            (Some(expected), None) => self.coerce_at(span, expected, &ty),
            (None, None) => {}
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

    /// The type of a capability operation `log.info` (spec 8.2): the declared operation type with
    /// the effect added to R. Effect type parameters are replaced with the capability value's
    /// type arguments (`get` of `use State[Int]` is `fn() -> Int`, spec 8.1).
    ///
    /// When `base` is a capability value captured by the closure or local fn being inferred, the
    /// R is the capability itself (`uses { primary }`, `[effect.use]`); it widens to the effect.
    /// An operation on a capability of `tags`. Only the operations of those effects can be
    /// called (`[effect.use]`); calling one adds its own effect (`[type.rules]`).
    fn infer_op(&mut self, expr: ExprId, base: ExprId, tags: &[Tag], name: &Label) -> Ty {
        let program = self.program;
        // `fs.Open(p)` opens a handler member of the handler that provides `fs`
        // (`[effect.use]`).
        if name.name.starts_with(|c: char| c.is_ascii_uppercase()) {
            return self.infer_cap_member(expr, base, tags, name);
        }
        // A label has the operations of its base (issue #255).
        let found = tags.iter().find_map(|t| {
            program
                .effect_ops(t.def)
                .iter()
                .copied()
                .find(|&op| program.defs[op].name == name.name)
                .map(|op| (t, op))
        });
        let Some((tag, op)) = found else {
            return self.field_error(
                name,
                DiagnosticKind::UnknownOperation {
                    effect: cap_effects_name(program, tags),
                    op: name.name.clone(),
                },
            );
        };
        self.ops.insert(expr, op);
        // The effect's type parameters come first in the operation's scheme, followed by the
        // operation's own (`start[A]`, spec 8.1), which are instantiated per reference. Through a
        // label, they are the base's type arguments (`effect Hits of State[Int]`).
        let args = self.sigs.base_of(tag).args.clone();
        let mut receiver = None;
        let ty = self.sigs.get(op).map(|sig| {
            let n = args.len().min(sig.scheme.params.len());
            let (fresh, fresh_rows) = self.ctx.fresh_args(&sig.scheme.params[n..]);
            let mut tys = args[..n].to_vec();
            let mut rows: Vec<Row> = tys
                .iter()
                .enumerate()
                .map(|(i, a)| match a {
                    Ty::Set(_, row) => row.clone(),
                    _ => Row::from_elems([RowElem::Bound(i as u32)]),
                })
                .collect();
            if !fresh.is_empty() {
                self.type_args.insert(expr, fresh.clone());
            }
            // The set of the receiver (`[effect.handler]`, `op_signature`): the capability the
            // operation is called on.
            if sig
                .scheme
                .params
                .last()
                .is_some_and(|p| p.name == "C(self)")
                && let Some(row) = fresh_rows.last()
            {
                receiver = Some(row.clone());
            }
            tys.extend(fresh);
            rows.extend(fresh_rows);
            substitute(&sig.scheme.ty, &tys, &rows)
        });
        if let Some(row) = receiver
            && let Some(Ty::Cap(_, caps)) =
                self.exprs.get(base).map(|t| self.ctx.shallow_resolve(t))
        {
            self.ctx
                .add_subset_of(SetKind::Caps, caps, row, Origin::Lambda);
        }
        // A capability held in a field of the handler outlives the handler: it belongs to a
        // `with` around the handler's, or to a handler member its init opened (`[effect.use]`,
        // `[effect.handler]`). It is not a requirement of the handler: its element is the
        // handler's `self`, which the end of the body removes. It still counts as its effect for
        // whether the body may suspend (E0417). In release, a suspend fn called that way makes
        // Async a requirement, so that leaving the `with` shows that it may suspend. The field
        // may have been bound to a local variable first (`p = self.pool`): its set is still the
        // field's (#866).
        let uses = match (self.field_capability(base), self.current) {
            (Some(field), Some(handler)) => {
                let in_release = match program.item(handler) {
                    Some(Item::Handler(h)) => h.release.is_some_and(|(_, body)| {
                        let (body, at) = (program.exprs[body].span, program.exprs[expr].span);
                        body.module == at.module && body.range.contains_range(at.range)
                    }),
                    _ => false,
                };
                let mut row = Row::from_elems([RowElem::Cap(CapElem {
                    id: CapId::SelfOf(handler),
                    name: SmolStr::new(field),
                    tag: tag.clone(),
                })]);
                let suspend = matches!(program.item(op), Some(Item::Op(o)) if o.sig.is_suspend);
                if in_release
                    && suspend
                    && let Some(async_) = program.prelude_def("Async")
                {
                    row.insert(RowElem::Tag(Tag::new(async_)));
                }
                row
            }
            // The capabilities the receiver may be (`[effect.use]`): each counts as the effect
            // of the operation for whether the call may suspend. The name of an effect is the
            // effect.
            _ => self.receiver_uses(base, tag),
        };
        match ty {
            // The capabilities passed to the operation's capability parameters are in R too. A
            // set of a field of the handler among them (`self.pool.read(self.id)`) is removed at
            // the end of the body, as the field's capability is (`end_self`, #866).
            Some(Ty::Fn(f)) => {
                let mut uses = uses;
                uses.extend(&f.uses);
                Ty::func_sets(f.params, f.ret, f.fails, uses)
            }
            _ => Ty::Error,
        }
    }

    /// The name of the effect that declares the handler member `member`.
    fn member_effect_name(&self, member: DefId) -> SmolStr {
        let program = self.program;
        program.defs[member]
            .parent
            .map(|e| program.defs[e].name.clone())
            .unwrap_or_default()
    }

    /// Whether `expr` is a capability held in a field of the handler, and its name: a field of
    /// the implicit `self` (`self.inner`), or a local variable bound to one (`p` after
    /// `p = self.inner`). The type of such a local variable has a set variable whose solution is
    /// not known yet, so the binding is followed instead.
    fn field_capability(&self, expr: ExprId) -> Option<SmolStr> {
        let program = self.program;
        match &program.exprs[expr].kind {
            ExprKind::Field { base, name } => match program.exprs[*base].kind {
                ExprKind::Path(Res::Local(l)) if program.locals[l].kind == LocalKind::SelfParam => {
                    Some(SmolStr::new(format!("self.{}", name.name)))
                }
                _ => None,
            },
            ExprKind::Path(Res::Local(l)) => self
                .let_values
                .get(l)
                .and_then(|&value| self.field_capability(value)),
            _ => None,
        }
    }

    /// The sets of the fields of `handler` as they are in its operation bodies and release: the
    /// hidden set type parameters of the handler's type, `Bound(i)` after the written type
    /// parameters (`[effect.handler]`). They are decided at each item of a with that runs the
    /// init. The written set type parameters of a built-in handler (`Background[E, R]`) are not.
    pub(super) fn field_sets(&self, handler: DefId) -> Vec<RowElem> {
        let Some(caps) = self.ty_table.decl_caps(handler) else {
            return Vec::new();
        };
        let n = self.ty_table.self_ty(handler).map_or(0, |ty| match ty {
            Ty::Named(_, args) => args.len(),
            _ => 0,
        });
        (caps.visible..n)
            .map(|i| RowElem::Bound(i as u32))
            .collect()
    }

    /// Opening a handler member is written only as an item of `with` and, as a call, inside an
    /// init (E0448, `[effect.use]`, `[effect.handler]`). Records the opening.
    fn check_member_open(&mut self, expr: ExprId, member: DefId, span: Span) {
        self.member_opens.insert(expr, member);
        if self.in_init.is_none() || !self.member_callees.contains(&expr) {
            self.diags.push(Diagnostic {
                span: Some(span),
                kind: DiagnosticKind::MemberOutsideInit {
                    member: self.program.defs[member].name.clone(),
                    effect: self.member_effect_name(member),
                },
            });
        }
    }

    /// Records the innermost `with` inside the init that provides the handler `expr` opens
    /// `member` through: the capability `base`, or the name of its effect.
    fn record_open_with(&mut self, expr: ExprId, member: DefId, base: Option<ExprId>) {
        if self.init_withs.is_empty() {
            return;
        }
        let caps: Vec<CapElem> = match base {
            // A local's own type: a reference to it has a fresh set that contains it.
            Some(base) => match (&self.program.exprs[base].kind, self.exprs.get(base)) {
                (ExprKind::Path(Res::Local(l)), _) if self.locals.get(*l).is_some() => {
                    match self.locals.get(*l).map(|t| self.ctx.shallow_resolve(t)) {
                        Some(Ty::Cap(_, caps)) => {
                            self.ctx.resolve_row(&caps).caps().cloned().collect()
                        }
                        _ => Vec::new(),
                    }
                }
                (_, Some(t)) => match self.ctx.shallow_resolve(t) {
                    Ty::Cap(_, caps) => self.ctx.resolve_row(&caps).caps().cloned().collect(),
                    _ => Vec::new(),
                },
                _ => Vec::new(),
            },
            None => match self.program.defs[member].parent {
                Some(effect) => match self.anchor_name(&Tag::new(effect)) {
                    RowElem::Cap(c) => vec![c],
                    _ => Vec::new(),
                },
                None => Vec::new(),
            },
        };
        let with = self.init_withs.iter().rev().copied().find(|w| {
            caps.iter().any(|c| match c.id {
                CapId::Item { with, .. } => with == *w,
                CapId::Named { provider, .. } => provider == *w,
                _ => false,
            })
        });
        if let Some(with) = with {
            self.member_open_withs.insert(expr, with);
        }
    }

    /// The type `ty` of opening `member` at `expr`, with the capability opened recording the
    /// `with` inside the init that provides its provider, if any ([`Self::record_open_with`]):
    /// it cannot leave that `with` (`[effect.handler]`).
    fn opened_in_init(&mut self, expr: ExprId, member: DefId, ty: Ty) -> Ty {
        let Some(&with) = self.member_open_withs.get(expr) else {
            return ty;
        };
        let Ty::Fn(mut f) = ty else {
            return ty;
        };
        if let Ty::Cap(tags, _) = &f.ret
            && let Some(tag) = tags.first().cloned()
        {
            let name = self.program.defs[member].name.clone();
            f.ret = Ty::Cap(
                tags.clone(),
                Row::from_elems([RowElem::Cap(CapElem {
                    id: CapId::Item {
                        with,
                        item: crate::row::OPENED_IN_INIT,
                    },
                    name,
                    tag,
                })]),
            );
        }
        Ty::Fn(f)
    }

    /// The type of opening the handler member `name` through the capability `base` of `tag`
    /// (`fs.Open`, `[type.rules]`): the member's type with the effect's type parameters replaced
    /// by the capability's type arguments, and the capability itself as its R, as for an
    /// operation ([`Self::infer_op`]).
    fn infer_cap_member(&mut self, expr: ExprId, base: ExprId, tags: &[Tag], name: &Label) -> Ty {
        let program = self.program;
        let found = tags
            .iter()
            .find_map(|t| program.effect_member(t.def, &name.name).map(|m| (t, m)));
        let Some((tag, member)) = found else {
            return self.field_error(
                name,
                DiagnosticKind::UnknownOperation {
                    effect: cap_effects_name(program, tags),
                    op: name.name.clone(),
                },
            );
        };
        self.check_member_open(expr, member, name.span);
        self.record_open_with(expr, member, Some(base));
        let ty = self.cap_member_ty(base, tag, member);
        self.opened_in_init(expr, member, ty)
    }

    /// The type of opening `member` through the capability `cap` of `tag`.
    fn cap_member_ty(&mut self, cap: ExprId, tag: &Tag, member: DefId) -> Ty {
        let args = self.sigs.base_of(tag).args.clone();
        let Some(Ty::Fn(f)) = self
            .sigs
            .get(member)
            .map(|sig| substitute_bound(&sig.scheme.ty, &args))
        else {
            return Ty::Error;
        };
        let uses = self.receiver_uses(cap, tag);
        Ty::func_sets(f.params, f.ret, f.fails, uses)
    }

    /// What calling an operation of `tag` on the capability `base` adds to R: the capabilities
    /// it may be (`[effect.use]`). Each counts as `tag` for whether the call may suspend. The
    /// name of an effect is the effect itself in R.
    pub(super) fn receiver_uses(&mut self, base: ExprId, tag: &Tag) -> Row {
        match self.exprs.get(base).map(|t| self.ctx.shallow_resolve(t)) {
            Some(Ty::Cap(_, caps)) => Row::from_elems(caps.elems().iter().map(|e| match e {
                RowElem::Cap(c) if matches!(c.id, CapId::Named { .. }) => {
                    RowElem::Tag(c.tag.clone())
                }
                RowElem::Cap(c) => RowElem::Cap(CapElem {
                    tag: tag.clone(),
                    ..c.clone()
                }),
                e => e.clone(),
            })),
            _ => Row::of_tag(tag.clone()),
        }
    }

    /// The type of field `field` of type definition `type_def` applied to `args`. Fields come from
    /// the constructor's declared type (labels and parameter types).
    fn field_ty(&mut self, type_def: DefId, args: &[Ty], field: &SmolStr) -> FieldTy {
        let program = self.program;
        let ctor = match program.item(type_def) {
            Some(Item::Type(item)) => match item.ctor {
                Some(ctor) => ctor,
                None => return FieldTy::NotARecord,
            },
            // An error is its own constructor.
            Some(Item::Error(_)) => type_def,
            // A handler value's fields (`self.conn`, spec 8.4).
            Some(Item::Handler(item)) => match item.ctor {
                Some(ctor) => ctor,
                None => return FieldTy::NoSuchField,
            },
            _ => return FieldTy::NotARecord,
        };
        let Some(sig) = self.sigs.get(ctor) else {
            return FieldTy::NotARecord;
        };
        let Ty::Fn(f) = &sig.scheme.ty else {
            // A type or error without fields.
            return FieldTy::NoSuchField;
        };
        match sig.labels.iter().position(|l| l.as_ref() == Some(field)) {
            Some(i) => FieldTy::Found(i, substitute_bound(&f.params[i], args)),
            None => FieldTy::NoSuchField,
        }
    }

    /// Pushes a field diagnostic at the field name and returns `Ty::Error`.
    fn field_error(&mut self, name: &Label, kind: DiagnosticKind) -> Ty {
        self.diags.push(Diagnostic {
            span: Some(name.span),
            kind,
        });
        Ty::Error
    }

    /// The type of an update `C(f: e, ..base)` (spec 5.3).
    ///
    /// The constructor's type is instantiated twice: once for the value being built and once for
    /// `base`, and the type arguments may differ. Written fields are checked against the first;
    /// unwritten fields are carried over from `base`, so their types in the two instances are
    /// unified.
    ///
    /// In the reordered arguments, only written fields are `Some`; unwritten ones are `None`
    /// (carried over from `base`).
    fn infer_update(&mut self, expr: ExprId, ctor: ExprId, fields: &[Arg], base: ExprId) -> Ty {
        let program = self.program;
        let sigs = self.sigs;
        let ctor_ty = self.infer_expr(ctor);
        let ExprKind::Path(Res::Def(ctor_def)) = program.exprs[ctor].kind else {
            // Name resolution has reported non-constructors.
            for field in fields {
                self.infer_expr(field.value);
            }
            self.infer_expr(base);
            return Ty::Error;
        };
        let Ty::Fn(made) = self.ctx.shallow_resolve(&ctor_ty) else {
            // A constructor without fields (or without a declared type).
            self.check_expr(base, &ctor_ty);
            for field in fields {
                self.infer_expr(field.value);
            }
            return ctor_ty;
        };
        let Some(sig) = sigs.get(ctor_def) else {
            return Ty::Error;
        };
        let base_ty = self.ctx.instantiate(&sig.scheme);
        let Ty::Fn(from) = base_ty else {
            return Ty::Error;
        };
        self.check_expr(base, &from.ret);

        let mut written = vec![false; made.params.len()];
        let mut slots: Vec<Option<ExprId>> = vec![None; made.params.len()];
        for field in fields {
            let Some(label) = &field.label else {
                // Name resolution has reported fields written by position.
                self.infer_expr(field.value);
                continue;
            };
            let slot = sig
                .labels
                .iter()
                .position(|l| l.as_ref() == Some(&label.name));
            match slot {
                Some(i) if written[i] => {
                    self.diags.push(Diagnostic {
                        span: Some(label.span),
                        kind: DiagnosticKind::DuplicateArg {
                            label: label.name.clone(),
                        },
                    });
                    self.infer_expr(field.value);
                }
                Some(i) => {
                    written[i] = true;
                    slots[i] = Some(field.value);
                    self.check_expr(field.value, &made.params[i]);
                }
                None => {
                    let ty = self.ctx.resolve(&made.ret);
                    self.diags.push(Diagnostic {
                        span: Some(label.span),
                        kind: DiagnosticKind::NoField {
                            ty,
                            field: label.name.clone(),
                        },
                    });
                    self.infer_expr(field.value);
                }
            }
        }

        // Unwritten fields are carried over from base.
        let base_span = program.exprs[base].span;
        for (i, was_written) in written.iter().enumerate() {
            if !was_written {
                self.coerce_at(base_span, &made.params[i], &from.params[i]);
            }
        }
        self.calls.insert(expr, slots);

        made.ret
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
    /// Every arm below an arm `_` without a guard is unreachable (W0301), whatever the E of the
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

    /// Also records the types of the variables the pattern binds.
    ///
    /// `ty` is the type of the incoming value, so it is the expected side and the pattern's type
    /// the actual side: a mismatch reads as the pattern not fitting the value. Patterns that
    /// cannot be placed (an extra field) get `Ty::Error`.
    fn bind_pat(&mut self, pat: PatId, ty: &Ty) {
        self.pats.insert(pat, ty.clone());

        let program = self.program;
        let p = &program.pats[pat];
        match &p.kind {
            PatKind::Bind(local) => {
                self.set_local(*local, ty.clone());
            }
            PatKind::Unit => self.coerce_at(p.span, ty, &Ty::UNIT),
            PatKind::Lit(lit) => {
                let lit_ty = self.lit_ty(lit, p.span);
                self.coerce_at(p.span, ty, &lit_ty);
            }
            PatKind::Const(Res::Def(def)) => {
                let const_ty = self.infer_def_ref(*def, p.span);
                self.coerce_at(p.span, ty, &const_ty);
            }
            PatKind::Const(_) => {}
            PatKind::Tuple(elems) => {
                let elem_tys: Vec<Ty> = elems.iter().map(|_| self.ctx.new_var()).collect();
                self.coerce_at(p.span, ty, &Ty::tuple(elem_tys.clone()));
                for (elem, elem_ty) in elems.iter().zip(&elem_tys) {
                    self.bind_pat(*elem, elem_ty);
                }
            }
            PatKind::List { elems, rest } => {
                let elem_ty = self.ctx.new_var();
                self.coerce_at(p.span, ty, &Ty::list(elem_ty.clone()));

                for elem in elems {
                    self.bind_pat(*elem, &elem_ty);
                }

                if let Some(ListRest {
                    binding: Some(local),
                    ..
                }) = rest
                {
                    self.set_local(*local, Ty::list(elem_ty));
                }
            }
            PatKind::Ctor {
                res: Res::Def(def),
                args,
                rest,
            } => self.bind_ctor_pat(pat, *def, args.as_deref(), *rest, ty),
            PatKind::Ctor { .. } => {}
            PatKind::Missing => {}
            PatKind::Wild => {}
        }
    }

    /// Constructor patterns: instantiates the constructor's type, checks the constructed type
    /// against `ty`, and passes types to the fields. Fields are reordered like call arguments;
    /// omitting fields without `..` is E0308 (spec 6.5).
    fn bind_ctor_pat(
        &mut self,
        pat: PatId,
        def: DefId,
        args: Option<&[PatArg]>,
        rest: bool,
        ty: &Ty,
    ) {
        let span = self.program.pats[pat].span;
        let sigs = self.sigs;
        let program = self.program;

        let ctor_ty = self.infer_def_ref(def, span);
        let labels = sigs.get(def).map(|sig| sig.labels.as_slice());

        let (fields_tys, made_ty) = match self.ctx.shallow_resolve(&ctor_ty) {
            Ty::Fn(f) => (f.params, f.ret),
            other => (Vec::new(), other),
        };

        let ctor_name = &program.defs[def].name;
        let pat_args = args.unwrap_or(&[]);
        let assigned = self.assign_args(
            Some(ctor_name),
            labels,
            fields_tys.len(),
            pat_args.iter().map(|arg| (arg.label.as_ref(), arg.pat)),
        );

        if !assigned.reported {
            if assigned.too_many {
                self.diags.push(Diagnostic {
                    span: Some(span),
                    kind: DiagnosticKind::PatternFieldCount {
                        ctor: ctor_name.clone(),
                        expected: assigned.slots.len(),
                        found: pat_args.len(),
                    },
                });
            } else if assigned.has_missing() && !rest {
                // Collect every unfilled named field; positional fields have no name.
                let missing_fields: Vec<SmolStr> = assigned
                    .slots
                    .iter()
                    .zip(labels.unwrap_or(&[]))
                    .filter(|(slot, _)| slot.is_none())
                    .filter_map(|(_, label)| label.clone())
                    .collect();

                if !missing_fields.is_empty() {
                    self.diags.push(Diagnostic {
                        span: Some(span),
                        kind: DiagnosticKind::MissingPatternFields {
                            ctor: ctor_name.clone(),
                            fields: missing_fields,
                        },
                    })
                } else {
                    self.diags.push(Diagnostic {
                        span: Some(span),
                        kind: DiagnosticKind::PatternFieldCount {
                            ctor: ctor_name.clone(),
                            expected: assigned.slots.len(),
                            found: pat_args.len(),
                        },
                    })
                }
            }
        }

        for (slot, field_ty) in assigned.slots.iter().zip(&fields_tys) {
            if let Some(pat) = slot {
                self.bind_pat(*pat, field_ty);
            }
        }

        for extra in assigned.extra {
            self.bind_pat(extra, &Ty::Error);
        }
        self.pat_fields.insert(pat, assigned.slots);

        self.coerce_at(span, ty, &made_ty);
    }

    /// Reorders arguments by parameter position: positional arguments from the front, named
    /// arguments by label. Used both for call arguments (`ExprId`) and constructor pattern
    /// arguments (`PatId`).
    ///
    /// `labels` are the callee's parameter labels, or `None` for a value not referred to by name
    /// (a local variable, a lambda). Label errors are reported here; count errors are reported by
    /// the caller, since calls and patterns have different rules.
    fn assign_args<'l, T: Copy>(
        &mut self,
        callee_name: Option<&SmolStr>,
        labels: Option<&[Option<SmolStr>]>,
        n_params: usize,
        args: impl IntoIterator<Item = (Option<&'l Label>, T)>,
    ) -> Assigned<T> {
        let mut assigned = Assigned {
            slots: vec![None; n_params],
            extra: Vec::new(),
            reported: false,
            too_many: false,
        };
        let mut next_positional = 0;

        for (label, value) in args {
            match label {
                None => {
                    if next_positional < n_params {
                        assigned.slots[next_positional] = Some(value);
                    } else {
                        assigned.extra.push(value);
                        assigned.too_many = true;
                    }
                    next_positional += 1;
                }
                Some(label) => {
                    match label_slot(callee_name, labels, &assigned.slots, &label.name) {
                        Ok(i) => assigned.slots[i] = Some(value),
                        Err(kind) => {
                            self.diags.push(Diagnostic {
                                span: Some(label.span),
                                kind,
                            });
                            assigned.reported = true;
                            assigned.extra.push(value);
                        }
                    }
                }
            }
        }

        assigned
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

/// Follows unification bindings for every type in the table.
fn resolve_all<T>(ctx: &mut InferCtx, map: &SparseMap<Idx<T>, Ty>) -> SparseMap<Idx<T>, Ty> {
    let mut out = SparseMap::default();
    for (id, ty) in map.iter() {
        out.insert(id, ctx.resolve(ty));
    }
    out
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

/// The position for a named argument, or the diagnostic explaining why it cannot be placed.
fn label_slot<T>(
    callee_name: Option<&SmolStr>,
    labels: Option<&[Option<SmolStr>]>,
    slots: &[Option<T>],
    label: &SmolStr,
) -> Result<usize, DiagnosticKind> {
    let labels = labels.ok_or_else(|| DiagnosticKind::LabelOnUnnamedCallee {
        label: label.clone(),
    })?;

    let i = labels
        .iter()
        .position(|l| l.as_ref() == Some(label))
        .ok_or_else(|| DiagnosticKind::UnknownLabel {
            callee: callee_name.cloned().unwrap_or_default(),
            label: label.clone(),
        })?;

    if slots[i].is_some() {
        return Err(DiagnosticKind::DuplicateArg {
            label: label.clone(),
        });
    }

    Ok(i)
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

fn without_set_vars(ty: &Ty) -> Ty {
    let ty = ty.map_children(without_set_vars);
    match ty {
        Ty::Fn(mut f) => {
            let keep = |e: &RowElem| match e {
                RowElem::Var(_) => Row::empty(),
                e => Row::from_elems([e.clone()]),
            };
            f.fails = f.fails.flat_map(keep);
            f.uses = f.uses.flat_map(keep);
            Ty::Fn(f)
        }
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

/// The `Self` of a trait function determined only by its result type (E0333).
struct SelfObligation {
    ty: Ty,
    trait_def: DefId,
    /// The span of the reference.
    span: Span,
}

/// Whether `ty` mentions the type parameter `Ty::Bound(index)`.
fn mentions_bound(ty: &Ty, index: u32) -> bool {
    if let Ty::Bound(i) = ty {
        return *i == index;
    }
    let mut found = false;
    ty.for_each_child(|child| found |= mentions_bound(child, index));
    found
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
