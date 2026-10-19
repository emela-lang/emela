//! Type inference diagnostics. Unification failures ([`crate::TypeError`]) get a span and land
//! here.
//!
//! The driver decides the wording and error codes; this module only holds the contents.

use emela_resolve::hir::Span;
use smol_str::SmolStr;

use crate::error::TypeError;
use crate::ty::Ty;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// `None` for types inside Prelude declarations, which have no source.
    pub span: Option<Span>,
    pub kind: DiagnosticKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticKind {
    /// A type does not satisfy a trait (E0326, spec 10.1–10.3): a constraint at a call, or a
    /// supertrait at an impl. `ty` is displayed.
    NotImplemented { ty: SmolStr, trait_name: SmolStr },
    /// A `derive` of a trait the compiler has no derivation for (E0326, spec 10.5).
    NotDerivable { trait_name: SmolStr },
    /// A derived impl of a type that is not `pub`, which nothing requires (W0303, spec 10.5). A
    /// warning.
    UnusedDerive { trait_name: SmolStr, ty: SmolStr },
    /// A trait is its own supertrait (E0329, spec 10.1). `cycle` starts and ends with the same
    /// trait.
    SupertraitCycle { cycle: Vec<SmolStr> },
    /// A second impl of the same trait for the same type (E0330, spec 10.2). `ty` is the second
    /// impl's head.
    DuplicateImpl { ty: SmolStr, trait_name: SmolStr },
    /// An impl outside the modules of its type and its trait (E0331, spec 10.2).
    OrphanImpl { ty: SmolStr, trait_name: SmolStr },
    /// The `Self` of a trait function that only its result type determines (`Parse.parse(s)`)
    /// is left undetermined in the function (E0333, spec 10.3).
    AmbiguousSelf { trait_name: SmolStr },
    /// An impl function whose parameter count differs from the trait's declaration (E0332).
    ImplFnParamCount {
        name: SmolStr,
        trait_name: SmolStr,
        expected: usize,
        found: usize,
    },
    /// E0304. `List[Int, Int]`, or `Option` with no type arguments.
    WrongTypeArgCount {
        name: SmolStr,
        expected: usize,
        found: usize,
    },
    /// Syntax not supported yet. `what` names the syntax (`Self`, ...).
    Unsupported { what: &'static str },
    /// A unification failure (E0301 type mismatch, E0302 infinite type).
    Mismatch(TypeError),
    /// E0305. `callee` is the name of the function or constructor called, or `None` when calling
    /// a value not referred to by name (a local variable, a lambda).
    WrongArgCount {
        callee: Option<SmolStr>,
        expected: usize,
        found: usize,
    },
    /// E0306.
    UnknownLabel { callee: SmolStr, label: SmolStr },
    /// E0307. Includes naming the same parameter by position and by label.
    DuplicateArg { label: SmolStr },
    /// A named argument in a call of a value without labels (a local variable, a lambda) (E0306).
    LabelOnUnnamedCallee { label: SmolStr },
    /// E0309. Too many fields (`Rect(w, h, d)`), or too few without `..` for a constructor with
    /// positional fields (`Rect(w)`).
    PatternFieldCount {
        ctor: SmolStr,
        expected: usize,
        found: usize,
    },
    /// Fields omitted without `..` in a pattern of a constructor with named fields (E0308, spec
    /// 6.5). `fields` lists all omitted fields in declaration order.
    MissingPatternFields { ctor: SmolStr, fields: Vec<SmolStr> },
    /// E0310. With `integer`, Int or Int64 is required (integer literals); otherwise Int, Int64
    /// or Float (arithmetic operators, spec 6.6).
    NotNumeric { found: Ty, integer: bool },
    /// E0311, spec 2.4.
    IntLiteralOutOfRange { literal: SmolStr, ty: Ty },
    /// E0312. Used for field access and update.
    NoField { ty: Ty, field: SmolStr },
    /// Field access on a type without fields: enums, primitives, tuples, ... (E0313).
    NoFields { ty: Ty, field: SmolStr },
    /// Field access on a value whose type is not known yet (E0314). Needs a type annotation.
    FieldOnUnknownType { field: SmolStr },
    /// Field access on an opaque type outside its defining module (E0315, spec 4.4).
    OpaqueField { ty: Ty, field: SmolStr },
    /// A top-level fn parameter without a type (E0316, spec 6.1).
    MissingParamType { param: SmolStr },
    /// E0303, spec 6.5. `missing` holds examples of missing values (`Rect(_, _)`).
    NonExhaustiveMatch { missing: Vec<SmolStr> },
    /// A binding pattern that can fail (E0317, spec 6.3). `missing` holds examples of values that
    /// do not match.
    RefutableBinding { missing: Vec<SmolStr> },
    /// An arm fully covered by the arms above it (W0301, spec 6.5). A warning.
    UnreachableArm,
    /// Errors of the body not in the declared E (E0318, spec 7.4). `errors` lists them (tags or
    /// names of set type parameters). `declared` tells whether a `fails` clause was written; if
    /// not, the declaration has no errors and these are unhandled. A const's value cannot fail
    /// either (spec 5.6; `declared` is false).
    UnhandledErrors {
        errors: Vec<SmolStr>,
        declared: bool,
    },
    /// A non-error value passed to `fail`, or a single-error type parameter instantiated with a
    /// non-error type (E0319, spec 7.2).
    NotAnError { found: Ty },
    /// An `escape` arm pattern that is not an error constructor (E0320, spec 7.3).
    EscapeArmNotError,
    /// A function value's E not contained in the expected function type's E (E0321, spec 7.4,
    /// 9.2). `expected` and `found` are displayed sets (`Timeout | E`, or `{}` if empty).
    ErrorSetMismatch { expected: SmolStr, found: SmolStr },
    /// An `escape` arm catching an error the expression cannot raise (W0302, spec 7.3). A warning.
    UnraisedError { error: SmolStr },
    /// A label item of `with` (`with L`, `L by P`) whose provided effect the body never uses
    /// (W0401, issue #255). `label` is the label written. Only when the body's R is fully known
    /// (no sets with unknown contents).
    UnusedLabelProvider { label: SmolStr },
    /// The capability's effect has no operation of that name (E0401, spec 8.2).
    UnknownOperation { effect: SmolStr, op: SmolStr },
    /// The body's R not contained in the R allowed by the declaration or context (E0402, spec
    /// 8.3, 9.3, 13.3). `effects` lists the effects not contained (tags or names of set type
    /// parameters). `scope` names the scope function (`Fiber.scope`) when some of the effects are
    /// required by fibers forked inside it; those must be provided outside it (spec 11.2).
    UnhandledEffects {
        effects: Vec<SmolStr>,
        context: EffectContext,
        scope: Option<SmolStr>,
    },
    /// A function value's R not contained in the expected function type's R (E0403, spec 9.2).
    /// `expected` and `found` are displayed effect sets (`{ Logger, Clock }`, or `{}` if empty).
    EffectSetMismatch { expected: SmolStr, found: SmolStr },
    /// E0404, spec 8.4. `ops` lists the missing operations in declaration order.
    MissingHandlerOps { handler: SmolStr, ops: Vec<SmolStr> },
    /// E0405, spec 8.4.
    DuplicateHandlerOp { op: SmolStr },
    /// A handler operation's parameter count differs from the effect's declaration (E0406,
    /// spec 8.4).
    HandlerOpArgCount {
        op: SmolStr,
        expected: usize,
        found: usize,
    },
    /// An `@external` fn with `use` omitted, or an `@external suspend fn`, used outside a handler
    /// operation's body (E0407, spec 12.2). `suspend` is true for the latter (the note differs).
    ExternalOutsideHandler { name: SmolStr, suspend: bool },
    /// E0408, spec 8.2.
    CapabilityArgument,
    /// A capability value put in a data structure (list, tuple, constructor) (E0409, spec 8.2).
    CapabilityInData,
    /// E0410, spec 8.2.
    CapabilityReturned,
    /// Handlers in the same `with` or layer depend on each other's effects in a cycle (E0411,
    /// spec 8.3, 8.5, 14.2). `cycle` names the handlers along the cycle, repeating the first one
    /// at the end.
    HandlerCycle { cycle: Vec<SmolStr> },
    /// Consts whose values certainly evaluate each other (E0231, spec 5.6). `cycle` names the
    /// consts along the cycle, repeating the first one at the end.
    ConstCycle { cycle: Vec<SmolStr> },
    /// A handler value's constructor used outside that handler's `init` (E0418, spec 8.4).
    HandlerCtorOutsideInit { handler: SmolStr },
    /// A handler with fields but no `init` (E0419, spec 8.4).
    MissingInit { handler: SmolStr },
    /// E0420, spec 8.4. `errors` lists the errors the release can raise.
    ReleaseFails {
        handler: SmolStr,
        errors: Vec<SmolStr>,
    },
    /// Two handlers in a layer provide the same effect (E0421, spec 8.5). `first` is the one
    /// listed first. If the same handler is listed twice, `first` and `second` are the same.
    DuplicateLayerEffect {
        layer: SmolStr,
        effect: SmolStr,
        first: SmolStr,
        second: SmolStr,
    },
    /// Layers include each other in a cycle (E0422, spec 8.5). `cycle` names the layers along the
    /// cycle, repeating the first one at the end.
    LayerCycle { cycle: Vec<SmolStr> },
    /// The base of a label cannot be used (E0429, issue #255): another label, or a built-in
    /// effect (Async, Scope).
    LabelBase {
        label: SmolStr,
        base: SmolStr,
        reason: LabelBaseReason,
    },
    /// An effect that is not a label written where one is needed (E0430, issue #255): listed in
    /// `with` (`with Console`), or left of `by` (`Console by H`).
    NotALabel { name: SmolStr, by: bool },
    /// The provider plugged into a label provides another effect (E0431, issue #255).
    /// `provided` is what `provider` provides.
    LabelMismatch {
        label: SmolStr,
        base: SmolStr,
        provider: SmolStr,
        provided: SmolStr,
    },
    /// The same label is given a provider twice in one `with` (E0432, issue #255).
    LabelProvidedTwice { label: SmolStr },
    /// A handler written for a label (E0433, issue #255). A label takes a handler of its base
    /// with `by`.
    HandlerOfLabel {
        handler: SmolStr,
        label: SmolStr,
        base: SmolStr,
    },
    /// A handler written for the built-in Async effect (E0413, spec 8.7).
    AsyncHandler { handler: SmolStr },
    /// A handler written for a built-in effect such as Scope (E0413, spec 11.2). `provider` names
    /// the function that provides it (`Fiber.scope`).
    IntrinsicHandler {
        handler: SmolStr,
        effect: SmolStr,
        provider: SmolStr,
    },
    /// The value type of a scope function (`Fiber.scope`) contains a fiber handle or a Deferred
    /// (E0425, spec 11.8). `ty` names the type (`Fiber`, `Deferred`); `scope` names the function.
    EscapingFromScope { ty: SmolStr, scope: SmolStr },
    /// Type parameters on an operation of a user effect (E0426, spec 8.1). Only built-in effects
    /// may have them.
    OpTypeParams { op: SmolStr },
    /// `use Async` written (E0414, spec 8.7). Async has no operations; only the runtime provides
    /// it.
    UseAsync,
    /// A handler operation's `suspend` differs from the effect's declaration (E0415, spec 8.1,
    /// 8.4). `suspend` is whether the effect declares it as `suspend fn`.
    SuspendMismatch {
        op: SmolStr,
        effect: SmolStr,
        suspend: bool,
    },
    /// The body of an implementation of a non-suspend operation contains a call that may suspend
    /// (E0417, spec 8.1). `op` names the implementation; `call` says why the call may suspend.
    /// Reported once per call.
    SuspendingFnOp { op: SmolStr, call: SuspendingCall },
    /// An effect set required to be `Immediate` contains an effect that may suspend, or a set not
    /// known to be Immediate (E0416, spec 9.5). `effects` lists the effects that may suspend
    /// (those with a suspend fn, or Async); `sets` names set type parameters without an
    /// `Immediate` bound; `required` says what required it.
    NotImmediate {
        effects: Vec<SmolStr>,
        sets: Vec<SmolStr>,
        required: ImmediateSource,
    },
    /// `Immediate` on a type parameter that is not an effect set (E0322, spec 9.5).
    ImmediateOnNonEffect { param: SmolStr },
    /// A set (`{ Db }`, `A | B`) in a non-set type argument position (E0324, spec 9.2).
    SetAsType,
    /// An effect name in a non-set type argument position (E0221, spec 8.2). Name resolution lets
    /// effect names through in type argument positions, so type inference checks this.
    EffectAsType { name: SmolStr },
    /// Missing type arguments of an effect with type parameters (E0423, spec 8.1): `use State`
    /// and `handler H: State`. `handler` names the handler in the handler case; `params`
    /// names the effect's type parameters.
    MissingEffectTypeArgs {
        effect: SmolStr,
        params: Vec<SmolStr>,
        handler: Option<SmolStr>,
    },
    /// An effect type parameter used as a set (E0424, spec 8.1). Effect type parameters are
    /// ordinary types only.
    EffectParamNotType { effect: SmolStr, param: SmolStr },
    /// Something other than a tag of the right kind or a set type parameter in a set type
    /// argument of a named type (the `E` of `Handler[E, R]`) (E0323, spec 9.2). `effects` is
    /// whether it is an effect-set position; `found` describes what was written.
    SetArgMismatch {
        param: SmolStr,
        ty: SmolStr,
        effects: bool,
        found: SmolStr,
    },
}

/// Why a call in the body of an fn operation implementation may suspend
/// ([`DiagnosticKind::SuspendingFnOp`], spec 8.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuspendingCall {
    /// A call of a suspend fn operation.
    Op { effect: SmolStr, op: SmolStr },
    /// A call of an `@external suspend fn`.
    External { callee: SmolStr },
    /// A call of a function or function value whose R is not Immediate. `callee` is the name of
    /// the function or local variable called; `effects` lists the effects in R that may suspend
    /// (those with a suspend fn, or Async).
    Fn {
        callee: Option<SmolStr>,
        effects: Vec<SmolStr>,
    },
}

/// Why a label's base cannot be used (E0429).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelBaseReason {
    /// The base is a label itself, of the effect named. A label's base is always an effect with
    /// operations.
    Label(SmolStr),
    /// The base is a built-in effect that only the runtime or a scope function provides.
    BuiltIn,
}

/// What required `Immediate` ([`DiagnosticKind::NotImmediate`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImmediateSource {
    /// A bound `R: Immediate` on type parameter `param` of `callee`.
    Bound { param: SmolStr, callee: SmolStr },
    /// A callback passed to an external function (spec 9.5).
    ExternalCallback { callee: SmolStr },
}

/// Where the set that R must be contained in came from ([`DiagnosticKind::UnhandledEffects`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectContext {
    /// A `use` clause written on the fn (spec 9.3).
    Declared,
    /// A const's value, which cannot have effects (spec 5.6).
    Const,
    /// `main`, whose R must be empty (spec 8.3).
    Main,
    /// A test function, whose R must be empty (spec 13.3).
    Test { name: SmolStr },
}

impl DiagnosticKind {
    /// Otherwise an error.
    pub fn is_warning(&self) -> bool {
        matches!(
            self,
            DiagnosticKind::UnreachableArm
                | DiagnosticKind::UnraisedError { .. }
                | DiagnosticKind::UnusedLabelProvider { .. }
                | DiagnosticKind::UnusedDerive { .. }
        )
    }
}
