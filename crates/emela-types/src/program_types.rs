//! The result of type checking a program ([`crate::check_program`]).

use emela_resolve::hir::{DefId, ExprId, LocalId, PatId, Span};
use la_arena::ArenaMap;

use crate::diagnostic::Diagnostic;
use crate::layer::LayerInfo;
use crate::lower::TyTable;
use crate::row::{Row, RowVar, Tag};
use crate::signature::Signatures;
use crate::traits::Traits;
use crate::ty::{Ty, TyVar};

/// Tables keyed by HIR ids.
///
/// Lowering walks the HIR and looks up what it needs in these tables; the LSP hover reads the
/// same tables. There is no typed tree (THIR): it would be a second tree of nearly the same shape
/// as the HIR to maintain. Types have unification bindings followed. Type variables that were
/// never determined (an unused `None`, say) remain type variables; lowering treats them as `()`,
/// and no representation would behave differently. Inside a generic fn's body, its type
/// parameters appear as `Ty::Bound(i)`.
///
/// Lowering runs only when there are no errors, so the tables may lack entries for erroneous
/// expressions (the `calls` of a call with a type error, say).
pub struct ProgramTypes {
    pub ty_table: TyTable,
    /// Declared types of fns, constructors and consts. Definitions without a declared type get
    /// their inferred, generalized type.
    pub sigs: Signatures,
    pub exprs: ArenaMap<ExprId, Ty>,
    pub pats: ArenaMap<PatId, Ty>,
    pub locals: ArenaMap<LocalId, Ty>,
    /// The definition whose body a local variable belongs to.
    pub local_owner: ArenaMap<LocalId, DefId>,
    /// Field access expressions (`ExprKind::Field`) → the field index (constructor declaration
    /// order).
    pub fields: ArenaMap<ExprId, usize>,
    /// Call (`Call`), pipe (`Binary { op: Pipe }`) and update (`Update`) expressions → the
    /// arguments reordered by parameter (field) position. Pipes include the left operand first,
    /// and method calls (`methods`) the x of `x.f(a)`;
    /// their callee is the right side's callee if the right side is a call, else the right side
    /// itself. Updates have `Some` only for the fields written. Whether to evaluate in written
    /// order (`named::in_written_order`) is left to lowering.
    pub calls: ArenaMap<ExprId, Vec<Option<ExprId>>>,
    /// References to generic definitions (`Path`) → the instantiated type arguments, in the
    /// declaration's type parameter order. Also references to capability operations with their
    /// own type parameters (the `Field` of `tasks.start`): only the operation's own, not the effect's.
    pub type_args: ArenaMap<ExprId, Vec<Ty>>,
    /// References to generic definitions (`Path`) → the instantiated sets, in the declaration's
    /// type parameter order (empty at positions that are not set parameters). Unlike `type_args`,
    /// these hold the sets instantiating set type parameters. Lowering uses them to decide, per
    /// instantiation, whether a function polymorphic in R may suspend.
    pub type_rows: ArenaMap<ExprId, Vec<Row>>,
    /// In the bodies of definitions without a declared type: generalized set variable → index of
    /// the type parameter in the declared type (`sigs`). The bodies' expression types and sets
    /// (`exprs`, `effects`) still contain the generalized set variables as `RowElem::Var`.
    pub generalized_rows: ArenaMap<DefId, Vec<(RowVar, u32)>>,
    /// For a definition without a declared type: generalized type variables in its body → the
    /// index of the type parameter of the declared type. Body expression types keep generalized
    /// variables as `Ty::Var`; lowering uses this to treat the `A` of `use State[A]` in the body
    /// as a type parameter (the evidence table key, spec 8.1).
    pub generalized_tys: ArenaMap<DefId, Vec<(TyVar, u32)>>,
    /// Constructor patterns → the patterns reordered by field position; omitted fields are
    /// `None`.
    pub pat_fields: ArenaMap<PatId, Vec<Option<PatId>>>,
    /// The error set E of each expression (spec 9.4), with set variables replaced by their
    /// solutions. In the bodies of generalized definitions, the generalized set variables remain
    /// as `RowElem::Var`.
    pub errors: ArenaMap<ExprId, Row>,
    /// The effect set R of each expression (spec 9.4), with set variables replaced by their
    /// solutions as in `errors`.
    pub effects: ArenaMap<ExprId, Row>,
    /// References to capability operations (the `Field` expression of `log.info`) → the effect
    /// operation's definition.
    pub ops: ArenaMap<ExprId, DefId>,
    /// Openings of handler members (`[effect.handler]`, `[effect.with]`) → the member: the callee
    /// of `Files.Open(p)` or `fs.Open(p)` inside an init, and the capability expression of an item
    /// `x = fs.Open(p)` of `with`.
    pub members: ArenaMap<ExprId, DefId>,
    /// Handler members opened inside an init through a handler that a `with` inside the init
    /// provides → that `with`, at whose end they are released (`[effect.handler]`).
    pub member_open_withs: ArenaMap<ExprId, ExprId>,
    /// The `use E` expressions (W4401) and the effect aliases written as a value (E4404) whose
    /// help does not suggest a bound item: a bound item provides nothing by name, and the body
    /// of the `with` that provides E also uses E by name (`[effect.with]`).
    pub bound_item_redirects: Vec<Span>,
    /// Method calls (the `Field` callee of `x.f(a)`, `[type.rules]`) → the fn of the module of the
    /// type of x, or the trait function, that it calls. The callee has that function's
    /// instantiated type and its `type_args` and `type_rows`, as a `Path` to it would, and the
    /// `calls` of the call put x first, as a pipe does.
    pub methods: ArenaMap<ExprId, DefId>,
    /// A handler's dependencies (the union of the R of init, release and the operation bodies;
    /// "deps(H)" in spec 9.4). Only for inferred handlers.
    pub handler_uses: ArenaMap<DefId, Row>,
    /// The E of a handler's init, which goes into the E of `with` (spec 9.4). Only for inferred
    /// handlers.
    pub handler_fails: ArenaMap<DefId, Row>,
    /// `with` expressions → handlers in initialization order (spec 14.2). Layers are expanded, and
    /// for handlers of the same effect only the last one is kept (spec 8.3, 8.5). Release runs in
    /// reverse order.
    pub with_handlers: ArenaMap<ExprId, crate::layer::WithInfo>,
    /// Layer → its expansion (handlers in initialization order and the dependencies the layer
    /// leaves unmet).
    pub layers: ArenaMap<DefId, LayerInfo>,
    /// `with` expressions → per item, the arguments of init reordered by parameter
    /// (`[effect.handler]`). Empty for an item without them. `WithBind::args` refers to these.
    pub with_args: ArenaMap<ExprId, Vec<Vec<Option<ExprId>>>>,
    /// Arguments whose capability is passed as fewer effects (`[effect.use]`) → the effects of
    /// the parameter, in the order of [`Ty::Cap`]. Lowering keeps only those operation tables.
    pub narrowings: ArenaMap<ExprId, Vec<Tag>>,
    /// `self` passed as the capability of its handler (`[effect.handler]`): lowering makes the
    /// handler's value from the state.
    pub self_caps: rustc_hash::FxHashSet<ExprId>,
    /// Layers → per item, the arguments of init reordered by parameter. Only for layers with
    /// arguments.
    pub layer_args: ArenaMap<DefId, Vec<Vec<Option<ExprId>>>>,
    /// `with` expressions → per item, the type arguments of the handler with type parameters
    /// that supplies it (`[effect.with]`): of the right of `by` for `L by H`. Empty for other
    /// items. A type variable generalized into the enclosing definition stays a type variable,
    /// as in `exprs` (`generalized_tys`).
    pub with_ty_args: ArenaMap<ExprId, Vec<Vec<Ty>>>,
    /// Layers → per member, the type arguments of a handler with type parameters
    /// (`[effect.layer]`). Empty for other members.
    pub layer_ty_args: ArenaMap<DefId, Vec<Vec<Ty>>>,
    /// Supertraits and impls (spec 10.1–10.2).
    pub traits: Traits,
    pub diags: Vec<Diagnostic>,
}
