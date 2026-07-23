use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::ast::{
    BinaryOp, Block, BlockItem, Bound, EffectRow, EnumDecl, Expr, Extern, FieldBinding, Function,
    FunctionType, ImplDecl, MatchArm, Pattern, Program, TraitDecl, Type,
};
use crate::error::{Diagnostic, Error, Result, Span};
use crate::resolve::{FnEntry, FnTable, Resolved, display_path};

#[derive(Debug, Clone)]
pub(crate) struct TypedProgram {
    pub(crate) functions: Vec<TypedFunction>,
}

#[derive(Debug, Clone)]
pub(crate) struct TypedFunction {
    pub(crate) params: Vec<Type>,
    pub(crate) ret: Type,
    pub(crate) throws: Option<Type>,
    pub(crate) effects: EffectRow,
    /// The effect row the body actually requires — a subset of the declared
    /// `effects`. The lint for over-declared effects (specs 0023/0035)
    /// compares the two.
    pub(crate) body_effects: EffectRow,
}

/// What a recorded [`TypeEntry`] describes, which decides how the language
/// server's hover renders it (spec 0033).
#[derive(Debug, Clone)]
pub(crate) enum EntryKind {
    /// A name-introducing site — parameter, `let`, pattern binding — rendered
    /// as `name: Type`.
    Binding(String),
    /// Any checked expression, rendered as the type alone.
    Expr,
}

/// One span→type fact recorded while checking: the raw material for hover.
#[derive(Debug, Clone)]
pub(crate) struct TypeEntry {
    pub(crate) span: Span,
    pub(crate) ty: Type,
    pub(crate) kind: EntryKind,
}

/// Every fact recorded during one [`check_with_index`] run, in checking order.
/// A body that fails keeps the entries recorded before its error, so hover
/// still works on the checked prefix (spec 0033).
#[derive(Debug, Default)]
pub(crate) struct TypeIndex {
    pub(crate) entries: Vec<TypeEntry>,
}

#[derive(Debug, Clone)]
struct FunctionSig {
    /// Declared type parameters (spec 0014); empty for a non-generic function.
    type_params: Vec<String>,
    /// Trait bounds on those parameters (spec 0020); discharged at each call site
    /// once the type arguments are inferred.
    bounds: Vec<Bound>,
    params: Vec<Type>,
    ret: Type,
    throws: Option<Type>,
    effects: EffectRow,
}

impl FunctionSig {
    fn is_generic(&self) -> bool {
        !self.type_params.is_empty()
    }
}

impl FunctionSig {
    fn ty(&self) -> Type {
        Type::Function(FunctionType {
            params: self.params.clone(),
            ret: Box::new(self.ret.clone()),
            throws: self.throws.clone().map(Box::new),
            effects: self.effects.clone(),
        })
    }
}

/// A registered `extern fn` / `intrinsic fn` and its visibility domain (spec
/// 0037): an effect backing operation is visible only to sibling operations of
/// its effect; any other extern only within its declaring module.
#[derive(Debug, Clone)]
struct ExternSig {
    sig: FunctionSig,
    /// The declaring file's `module` header (`Extern::module`).
    module: Option<String>,
    /// The owning effect for an effect backing operation.
    effect_name: Option<String>,
    /// `true` for an `intrinsic fn` (spec 0021). A Core Prelude intrinsic is
    /// visible by bare name from every module (the prelude is imported
    /// everywhere); other intrinsics/platform functions stay module-private.
    is_intrinsic: bool,
}

/// A declared enum's variants, in declaration order (spec 0005).
#[derive(Debug, Clone)]
struct EnumInfo {
    /// The declaring module (spec 0020 orphan rule).
    module: Option<String>,
    /// Type parameters of a generic enum (spec 0028); empty for a plain enum.
    /// The variant field types below are stated in terms of these.
    type_params: Vec<String>,
    variants: Vec<VariantInfo>,
}

/// A declared record's fields, in declaration order (spec 0006). A generic
/// record (spec 0028) carries its type parameters, in terms of which the field
/// types are stated.
#[derive(Debug, Clone)]
struct RecordInfo {
    /// The declaring module (spec 0020 orphan rule).
    module: Option<String>,
    /// Type parameters of a generic record (spec 0028); empty for a plain record.
    type_params: Vec<String>,
    /// Fields in declaration order: name and declared type (which may reference
    /// the type parameters as `Type::Var`).
    fields: Vec<(String, Type)>,
}

/// The `Option` lang item (spec 0042): the enum bound by `@lang("option")`
/// (spec 0041) and its present/absent variant names. Lets bare `Some`/`None`
/// resolve without the compiler hard-coding those names or a dedicated `Option`
/// type — the variants are identified by arity, not by name (spec 0041 L3).
#[derive(Debug, Clone)]
struct OptionLangItem {
    /// The bound enum's name (e.g. `Option`).
    enum_name: String,
    /// The present variant: one field, the type parameter (e.g. `Some`).
    some: String,
    /// The absent variant: no fields (e.g. `None`).
    none: String,
}

/// Validates an `@lang("option")` enum's shape (spec 0042 O1) and extracts its
/// present/absent variant names. The role requires exactly one type parameter
/// and exactly two variants: one carrying that parameter (present) and one with
/// no fields (absent). Roles are identified by arity, not by variant name.
fn option_lang_item_from(decl: &EnumDecl) -> Result<OptionLangItem> {
    let shape_error = || {
        Error::diagnostic(Diagnostic::new("Invalid `option` lang item").label(
            decl.name_span.clone(),
            "`@lang(\"option\")` requires `enum E<T> { Present(T), Absent }` (spec 0042 O1)",
        ))
    };
    if decl.type_params.len() != 1 || decl.variants.len() != 2 {
        return Err(shape_error());
    }
    let param = &decl.type_params[0];
    let mut some = None;
    let mut none = None;
    for variant in &decl.variants {
        match variant.fields.as_slice() {
            [Type::Var(field)] if field == param && some.is_none() => {
                some = Some(variant.name.clone());
            }
            [] if none.is_none() => none = Some(variant.name.clone()),
            _ => return Err(shape_error()),
        }
    }
    match (some, none) {
        (Some(some), Some(none)) => Ok(OptionLangItem {
            enum_name: decl.name.clone(),
            some,
            none,
        }),
        _ => Err(shape_error()),
    }
}

/// A declared trait (spec 0020): the set of method signatures a type may satisfy.
#[derive(Debug, Clone)]
struct TraitInfo {
    module: Option<String>,
    methods: Vec<TraitMethodInfo>,
}

#[derive(Debug, Clone)]
struct TraitMethodInfo {
    name: String,
    /// Parameter types. For an argument-dispatched method these contain
    /// `Type::Var("Self")` in some position; for a return-dispatched method
    /// (spec 0047, e.g. `empty`) `Self` appears only in `ret`.
    params: Vec<Type>,
    ret: Type,
    throws: Option<Type>,
    effects: EffectRow,
    has_default: bool,
    /// `Self` appears only in the return type (spec 0047): the impl is chosen
    /// from the call's expected type, not from an argument.
    return_dispatch: bool,
}

/// A registered `impl Trait for Type` (spec 0020). `target` may contain the
/// impl's own type variables for a parameterized instance; `bounds` are the
/// requirements those variables must satisfy for the impl to apply.
#[derive(Debug, Clone)]
struct ImplInfo {
    target: Type,
    bounds: Vec<Bound>,
}

#[derive(Debug, Clone)]
struct VariantInfo {
    name: String,
    fields: Vec<Type>,
}

#[derive(Debug, Clone)]
struct ExprInfo {
    ty: Type,
    effects: EffectRow,
    /// The error type this expression may put on the throws channel (spec
    /// 0011), still unresolved at this point. `None` means non-throwing.
    throws: Option<Type>,
    span: Span,
}

/// The enclosing function's error/return contract, threaded into `?`/`throw`.
struct FnCtx<'a> {
    throws: &'a Option<Type>,
    /// Trait bounds on the enclosing definition's type parameters (spec 0020):
    /// parameter name -> the trait names it is bounded by. Used to allow trait
    /// method calls on a still-abstract type parameter.
    bounds: &'a HashMap<String, Vec<String>>,
    /// The qualifier of the enclosing function (spec 0037): its `module_path`
    /// (`[EffectName]` inside an effect operation), against which bare names
    /// resolve. Empty for a compilation-root function.
    module: &'a [String],
    /// The enclosing function/literal's declared `uses` row (spec 0037): the
    /// gate an effect-operation reference must pass (`check_effect_gate`).
    effects: &'a EffectRow,
    /// The declaring file's `module` header, if any: the visibility domain for
    /// bare `extern`/`intrinsic` references (spec 0037).
    declared_module: Option<&'a str>,
    /// The owning effect when the enclosing function is itself an effect
    /// operation: what lets it call sibling backing externs by bare name.
    effect_name: Option<&'a str>,
    /// `true` inside a `@test` function's body (spec 0040 T3): a throwing call
    /// or `throw` at a position where 0011 would demand `?`/`try` instead
    /// propagates to the test harness, leaving the ordinary throws channel.
    /// Nested function literals reset this to `false` (their bodies follow the
    /// normal rules).
    implicit_try: bool,
}

/// Type-checks `program`. When `require_main` is false the program is treated as
/// a library — every function, impl method, and declaration is still checked, but
/// the `main` entrypoint (spec 0003) is not required. This is what `check
/// --library` uses to compile-check a module that has no `main`.
///
/// Errors are collected per declaration (spec 0033): each registration item and
/// each function/method body is checked independently, so one broken function
/// doesn't hide errors in another. Within one body the first error wins. An
/// empty error list means the program is well-typed.
pub(crate) fn check(
    program: &Program,
    require_main: bool,
    platform_registry: &[emela_codegen::PlatformFn],
) -> (TypedProgram, Vec<Error>) {
    let (typed, _, errors) = check_inner(program, require_main, platform_registry, false);
    (typed, errors)
}

/// [`check`] with span→type recording on: additionally returns the
/// [`TypeIndex`] the language server's hover reads (spec 0033). The normal
/// compile paths use [`check`] and record nothing.
pub(crate) fn check_with_index(
    program: &Program,
    require_main: bool,
    platform_registry: &[emela_codegen::PlatformFn],
) -> (TypedProgram, TypeIndex, Vec<Error>) {
    check_inner(program, require_main, platform_registry, true)
}

fn check_inner(
    program: &Program,
    require_main: bool,
    platform_registry: &[emela_codegen::PlatformFn],
    record: bool,
) -> (TypedProgram, TypeIndex, Vec<Error>) {
    let mut errors = Vec::new();
    let mut effects_in_scope: HashSet<String> = program
        .effects
        .iter()
        .map(|decl| decl.name.clone())
        .collect();
    // Host interface capabilities (spec 0026) are valid effects even when
    // the host interface source does not declare an `effect` block.
    for entry in platform_registry {
        if entry.capability.starts_with("host.") {
            effects_in_scope.insert(entry.capability.clone());
        }
    }
    let mut checker = Checker {
        table: FnTable::build(program),
        sigs: Vec::new(),
        externs: HashMap::new(),
        enums: HashMap::new(),
        records: HashMap::new(),
        option_lang_item: None,
        traits: HashMap::new(),
        impls: Vec::new(),
        impls_by: HashMap::new(),
        method_owners: HashMap::new(),
        effects_in_scope,
        platform_registry,
        type_index: record.then(|| RefCell::new(Vec::new())),
    };
    checker.register_enums(program, &mut errors);
    checker.register_records(program, &mut errors);
    checker.validate_data_decls(program, &mut errors);
    checker.register_traits(program, &mut errors);
    checker.register_impls(program, &mut errors);
    checker.register_functions(program, &mut errors);
    checker.register_externs(program, &mut errors);
    if require_main && let Err(error) = checker.check_main(program) {
        errors.push(error);
    }
    let mut body_effects = Vec::new();
    for function in &program.functions {
        // A `@test` function's signature rules (spec 0040 T2/T5) are checked
        // alongside the body so every violation is reported (spec 0033).
        if function.is_test {
            check_test_signature(function, &mut errors);
        }
        // Collect every error (spec 0033) while keeping `body_effects` aligned
        // with `program.functions` for the zip below; a failed body has no
        // inferred effects, so a default row stands in (unused once errors are
        // reported and lowering is skipped).
        match checker.check_function(function) {
            Ok(effects) => body_effects.push(effects),
            Err(error) => {
                errors.push(error);
                body_effects.push(EffectRow::default());
            }
        }
    }
    // Method bodies (spec 0020), including defaults filled in by
    // `expand_trait_defaults`, are checked with `Self` bound to the target type.
    for decl in &program.impls {
        for method in &decl.methods {
            if let Err(error) = checker.check_impl_method(decl, method) {
                errors.push(error);
            }
        }
    }
    let typed = TypedProgram {
        functions: program
            .functions
            .iter()
            .zip(body_effects)
            .map(|(function, body_effects)| TypedFunction {
                params: function
                    .params
                    .iter()
                    .map(|param| param.ty.clone())
                    .collect(),
                ret: function.ret.clone(),
                throws: function.throws.clone(),
                effects: function.effects.clone(),
                body_effects,
            })
            .collect(),
    };
    let index = TypeIndex {
        entries: checker
            .type_index
            .take()
            .map(RefCell::into_inner)
            .unwrap_or_default(),
    };
    (typed, index, errors)
}

struct Checker<'a> {
    /// Suffix-resolution table over all top-level functions (spec 0018), shared
    /// in structure with lowering.
    table: FnTable,
    /// Each top-level function's signature, indexed in parallel with
    /// `Program::functions` (so `FnEntry::index` indexes it).
    sigs: Vec<FunctionSig>,
    /// Platform functions and intrinsics (`extern fn` / `intrinsic fn`, specs
    /// 0013/0021), keyed by bare name, each carrying its visibility domain
    /// (spec 0037): bare references resolve only within the declaring module,
    /// or — for an effect backing operation — from sibling operations.
    externs: HashMap<String, ExternSig>,
    enums: HashMap<String, EnumInfo>,
    /// Declared records (spec 0006), keyed by name: the type parameters (spec
    /// 0028) and fields in declaration order.
    records: HashMap<String, RecordInfo>,
    /// The `Option` lang item (spec 0042), if the Core Prelude bound one with
    /// `@lang("option")`. Drives bare `Some`/`None` resolution.
    option_lang_item: Option<OptionLangItem>,
    /// Declared traits (spec 0020), keyed by name.
    traits: HashMap<String, TraitInfo>,
    /// Registered impls, referenced by index from `impls_by`.
    impls: Vec<ImplInfo>,
    /// The unique impl for each (trait, type-head) pair; the orphan rule (spec
    /// 0020) guarantees at most one, so keying by the type's head is sound.
    impls_by: HashMap<(String, String), usize>,
    /// Method name -> the traits declaring it, for bare-name dispatch and
    /// collision detection (spec 0020).
    method_owners: HashMap<String, Vec<String>>,
    /// The effect names in scope (spec 0037), from `Program::effects`.
    effects_in_scope: HashSet<String>,
    /// The platform registry (standard + host-interface entries, spec 0026).
    platform_registry: &'a [emela_codegen::PlatformFn],
    /// Span→type recording for the language server's hover (spec 0033):
    /// `Some` only under [`check_with_index`]. `RefCell` because checking is
    /// `&self` throughout; the borrow never outlives one `record` call.
    type_index: Option<RefCell<Vec<TypeEntry>>>,
}

impl<'a> Checker<'a> {
    fn register_enums(&mut self, program: &Program, errors: &mut Vec<Error>) {
        for decl in &program.enums {
            if self.enums.contains_key(&decl.name) {
                errors.push(Error::diagnostic(Diagnostic::new("Duplicate enum").label(
                    decl.name_span.clone(),
                    format!("enum `{}` is already defined", decl.name),
                )));
                continue;
            }
            let mut variants = Vec::new();
            let mut seen = HashSet::new();
            for variant in &decl.variants {
                if !seen.insert(variant.name.clone()) {
                    errors.push(Error::diagnostic(
                        Diagnostic::new("Duplicate variant").label(
                            variant.name_span.clone(),
                            format!("variant `{}` is already defined", variant.name),
                        ),
                    ));
                    continue;
                }
                variants.push(VariantInfo {
                    name: variant.name.clone(),
                    fields: variant.fields.clone(),
                });
            }
            self.enums.insert(
                decl.name.clone(),
                EnumInfo {
                    module: decl.module.clone(),
                    type_params: decl.type_params.clone(),
                    variants,
                },
            );
        }
        // Bind lang items (specs 0041/0042): the enum tagged `@lang("option")`
        // becomes the `Option` type. The Core Prelude's binding is authoritative,
        // so bind it first (pass 1); a user binding of the same role (pass 2) is
        // the duplicate, reported against the user's code (spec 0041 L2). The
        // shape (O1) is validated in `option_lang_item_from`.
        for core_pass in [true, false] {
            for decl in &program.enums {
                if decl.lang_item.as_deref() != Some("option") {
                    continue;
                }
                let is_core = decl.module.as_deref() == Some(crate::prelude::CORE_MODULE);
                if is_core != core_pass {
                    continue;
                }
                match option_lang_item_from(decl) {
                    Ok(item) if self.option_lang_item.is_none() => {
                        self.option_lang_item = Some(item);
                    }
                    Ok(_) => errors.push(Error::diagnostic(
                        Diagnostic::new("Duplicate lang item").label(
                            decl.name_span.clone(),
                            "lang-item role `\"option\"` is already bound by the Core Prelude (spec 0041 L2)",
                        ),
                    )),
                    Err(error) => errors.push(error),
                }
            }
        }
    }

    /// Registers each `record` (spec 0006): its fields in declaration order.
    /// Runs after `register_enums` so a record/enum name collision is caught.
    fn register_records(&mut self, program: &Program, errors: &mut Vec<Error>) {
        for decl in &program.records {
            if self.records.contains_key(&decl.name) || self.enums.contains_key(&decl.name) {
                errors.push(Error::diagnostic(Diagnostic::new("Duplicate type").label(
                    decl.name_span.clone(),
                    format!("`{}` is already defined", decl.name),
                )));
                continue;
            }
            let mut fields = Vec::new();
            let mut seen = HashSet::new();
            for field in &decl.fields {
                if !seen.insert(field.name.clone()) {
                    errors.push(Error::diagnostic(Diagnostic::new("Duplicate field").label(
                        field.name_span.clone(),
                        format!("field `{}` is already defined", field.name),
                    )));
                    continue;
                }
                fields.push((field.name.clone(), field.ty.clone()));
            }
            self.records.insert(
                decl.name.clone(),
                RecordInfo {
                    module: decl.module.clone(),
                    type_params: decl.type_params.clone(),
                    fields,
                },
            );
        }
    }

    /// Validates every named type mentioned by data declarations, once both
    /// enums and records are registered: enum payloads (spec 0028) and record
    /// fields (spec 0006) may reference each other in either order.
    fn validate_data_decls(&self, program: &Program, errors: &mut Vec<Error>) {
        for decl in &program.enums {
            for variant in &decl.variants {
                for field in &variant.fields {
                    if let Err(error) = self.validate_type(field, &variant.name_span) {
                        errors.push(error);
                    }
                }
            }
        }
        for decl in &program.records {
            for field in &decl.fields {
                if let Err(error) = self.validate_type(&field.ty, &field.name_span) {
                    errors.push(error);
                }
            }
        }
    }

    /// Rejects a type that names an enum that was never declared, or applies a
    /// generic enum at the wrong arity (spec 0005/0028).
    fn validate_type(&self, ty: &Type, span: &Span) -> Result<()> {
        match ty {
            Type::Enum(name, args) => {
                // A capitalized name in type position parses as `Type::Enum`;
                // it may also resolve to a record (spec 0006). A generic record
                // (spec 0028) must be applied at its declared arity.
                if let Some(record) = self.records.get(name) {
                    if args.len() != record.type_params.len() {
                        return Err(Error::diagnostic(
                            Diagnostic::new("Wrong number of type arguments").label(
                                span.clone(),
                                format!(
                                    "record `{name}` takes {} type argument(s), got {}",
                                    record.type_params.len(),
                                    args.len()
                                ),
                            ),
                        ));
                    }
                    for arg in args {
                        self.validate_type(arg, span)?;
                    }
                    return Ok(());
                }
                let Some(info) = self.enums.get(name) else {
                    return Err(Error::diagnostic(Diagnostic::new("Unknown type").label(
                        span.clone(),
                        format!("`{name}` is not a declared enum, record, or built-in type"),
                    )));
                };
                if args.len() != info.type_params.len() {
                    return Err(Error::diagnostic(
                        Diagnostic::new("Wrong number of type arguments").label(
                            span.clone(),
                            format!(
                                "`{name}` takes {} type argument(s), got {}",
                                info.type_params.len(),
                                args.len()
                            ),
                        ),
                    ));
                }
                for arg in args {
                    self.validate_type(arg, span)?;
                }
                Ok(())
            }
            Type::Array(inner) => self.validate_type(inner, span),
            Type::Function(function) => {
                for param in &function.params {
                    self.validate_type(param, span)?;
                }
                self.validate_type(&function.ret, span)?;
                if let Some(throws) = &function.throws {
                    self.validate_type(throws, span)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Registers each `trait` (spec 0020): validates method dispatchability and
    /// records the signatures and which trait owns each method name. Errors are
    /// collected per method (spec 0033); the valid methods still register so
    /// impls and calls against them keep checking.
    fn register_traits(&mut self, program: &Program, errors: &mut Vec<Error>) {
        for decl in &program.traits {
            if self.traits.contains_key(&decl.name) {
                errors.push(Error::diagnostic(Diagnostic::new("Duplicate trait").label(
                    decl.name_span.clone(),
                    format!("trait `{}` is already defined", decl.name),
                )));
                continue;
            }
            let mut methods = Vec::new();
            let mut seen = HashSet::new();
            for m in &decl.methods {
                if !seen.insert(m.name.clone()) {
                    errors.push(Error::diagnostic(
                        Diagnostic::new("Duplicate method").label(
                            m.name_span.clone(),
                            format!("`{}` is declared more than once in `{}`", m.name, decl.name),
                        ),
                    ));
                    continue;
                }
                // Dispatchability (spec 0020, relaxed by 0047): `Self` must
                // appear in a parameter type (argument dispatch) or, failing
                // that, in the return type (return dispatch, e.g. `empty`).
                // A method that mentions `Self` nowhere cannot be declared.
                let mut param_vars = HashSet::new();
                for param in &m.params {
                    collect_type_vars(&param.ty, &mut param_vars);
                }
                let mut ret_vars = HashSet::new();
                collect_type_vars(&m.ret, &mut ret_vars);
                let self_in_params = param_vars.contains("Self");
                let self_in_ret = ret_vars.contains("Self");
                if !self_in_params && !self_in_ret {
                    errors.push(Error::diagnostic(
                        Diagnostic::new("Undispatchable trait method")
                            .label(
                                m.name_span.clone(),
                                format!(
                                    "`{}` must mention `Self` in a parameter or return type",
                                    m.name
                                ),
                            )
                            .help(
                                "A trait method selects its impl from an argument's type, \
                                 or (spec 0047) from the call's expected type.",
                            ),
                    ));
                    continue;
                }
                // Return dispatch when `Self` is only in the return type.
                let return_dispatch = !self_in_params;
                for param in &m.params {
                    if let Err(error) = self.validate_type(&param.ty, &m.name_span) {
                        errors.push(error);
                    }
                }
                if let Err(error) = self.validate_type(&m.ret, &m.name_span) {
                    errors.push(error);
                }
                self.method_owners
                    .entry(m.name.clone())
                    .or_default()
                    .push(decl.name.clone());
                methods.push(TraitMethodInfo {
                    name: m.name.clone(),
                    params: m.params.iter().map(|param| param.ty.clone()).collect(),
                    ret: m.ret.clone(),
                    throws: m.throws.clone(),
                    effects: m.effects.clone(),
                    has_default: m.default_body.is_some(),
                    return_dispatch,
                });
            }
            self.traits.insert(
                decl.name.clone(),
                TraitInfo {
                    module: decl.module.clone(),
                    methods,
                },
            );
        }
    }

    /// Registers each `impl Trait for Type` (spec 0020): the orphan rule, global
    /// uniqueness (coherence), signature match, and exhaustiveness. Method bodies
    /// are checked later, once every impl is known. Each impl is checked
    /// independently (spec 0033): a broken one is reported and skipped.
    fn register_impls(&mut self, program: &Program, errors: &mut Vec<Error>) {
        for decl in &program.impls {
            if let Err(error) = self.register_impl(decl) {
                errors.push(error);
            }
        }
    }

    fn register_impl(&mut self, decl: &ImplDecl) -> Result<()> {
        let Some(trait_info) = self.traits.get(&decl.trait_name).cloned() else {
            return Err(Error::diagnostic(Diagnostic::new("Unknown trait").label(
                decl.trait_span.clone(),
                format!("`{}` is not a declared trait", decl.trait_name),
            )));
        };
        // The impl's own bounds must name its type parameters and real traits.
        for bound in &decl.bounds {
            if !decl.type_params.contains(&bound.param) {
                return Err(Error::diagnostic(
                    Diagnostic::new("Unknown type parameter").label(
                        bound.span.clone(),
                        format!("`{}` is not a parameter of this impl", bound.param),
                    ),
                ));
            }
            for tr in &bound.traits {
                if !self.traits.contains_key(tr) {
                    return Err(Error::diagnostic(Diagnostic::new("Unknown trait").label(
                        bound.span.clone(),
                        format!("`{tr}` is not a declared trait"),
                    )));
                }
            }
        }
        let Some(target_key) = type_head_key(&decl.target) else {
            return Err(Error::diagnostic(
                Diagnostic::new("Invalid impl target").label(
                    decl.target_span.clone(),
                    "an impl target must be a named or built-in type",
                ),
            ));
        };
        self.validate_type(&decl.target, &decl.target_span)?;
        // Orphan rule (spec 0020): the impl must live in the trait's module or
        // the target type's owning module.
        let target_owner = self.type_owning_module(&decl.target);
        let coherent =
            decl.module == trait_info.module || target_owner.as_ref() == Some(&decl.module);
        if !coherent {
            return Err(Error::diagnostic(
                Diagnostic::new("Orphan impl")
                    .label(
                        decl.trait_span.clone(),
                        format!(
                            "`impl {} for {:?}` is not in the trait's or the type's module",
                            decl.trait_name, decl.target
                        ),
                    )
                    .help("Place the impl in the module defining the trait or the type."),
            ));
        }
        // Global uniqueness: at most one impl per (trait, type head).
        let key = (decl.trait_name.clone(), target_key);
        if self.impls_by.contains_key(&key) {
            return Err(Error::diagnostic(
                Diagnostic::new("Conflicting implementations").label(
                    decl.trait_span.clone(),
                    format!("`{}` is already implemented for this type", decl.trait_name),
                ),
            ));
        }
        // Signature match (strict) plus no unknown/duplicate methods.
        let mut subst = HashMap::new();
        subst.insert("Self".to_string(), decl.target.clone());
        let mut method_seen = HashSet::new();
        for m in &decl.methods {
            let Some(tmethod) = trait_info.methods.iter().find(|tm| tm.name == m.name) else {
                return Err(Error::diagnostic(Diagnostic::new("Unknown method").label(
                    m.name_span.clone(),
                    format!("`{}` is not a method of `{}`", m.name, decl.trait_name),
                )));
            };
            if !method_seen.insert(m.name.clone()) {
                return Err(Error::diagnostic(
                    Diagnostic::new("Duplicate method").label(
                        m.name_span.clone(),
                        format!("`{}` is implemented more than once", m.name),
                    ),
                ));
            }
            self.check_impl_sig(tmethod, m, &subst)?;
        }
        // Exhaustiveness: every method without a default must be provided.
        // Defaults are already synthesized into `decl.methods` by
        // `expand_trait_defaults`, so they count as provided.
        for tmethod in &trait_info.methods {
            if !tmethod.has_default && !decl.methods.iter().any(|m| m.name == tmethod.name) {
                return Err(Error::diagnostic(
                    Diagnostic::new("Incomplete impl")
                        .code("incomplete-impl")
                        .label(
                            decl.trait_span.clone(),
                            format!(
                                "missing method `{}` required by `{}`",
                                tmethod.name, decl.trait_name
                            ),
                        ),
                ));
            }
        }
        let idx = self.impls.len();
        self.impls.push(ImplInfo {
            target: decl.target.clone(),
            bounds: decl.bounds.clone(),
        });
        self.impls_by.insert(key, idx);
        Ok(())
    }

    /// Checks that an impl method's signature equals the trait's under
    /// `Self := target` (spec 0020, strict match).
    fn check_impl_sig(
        &self,
        tmethod: &TraitMethodInfo,
        m: &Function,
        subst: &HashMap<String, Type>,
    ) -> Result<()> {
        let expected_params: Vec<Type> = tmethod
            .params
            .iter()
            .map(|t| subst_type(t, subst))
            .collect();
        let actual_params: Vec<Type> = m.params.iter().map(|p| subst_type(&p.ty, subst)).collect();
        let expected_ret = subst_type(&tmethod.ret, subst);
        let actual_ret = subst_type(&m.ret, subst);
        let expected_throws = tmethod.throws.as_ref().map(|t| subst_type(t, subst));
        let actual_throws = m.throws.as_ref().map(|t| subst_type(t, subst));
        if expected_params != actual_params
            || expected_ret != actual_ret
            || expected_throws != actual_throws
            || tmethod.effects != m.effects
        {
            return Err(Error::diagnostic(
                Diagnostic::new("Impl signature mismatch")
                    .label(
                        m.name_span.clone(),
                        format!("`{}` does not match its declaration in the trait", m.name),
                    )
                    .help("Match the trait signature with `Self` set to the target type."),
            ));
        }
        Ok(())
    }

    /// Type-checks an impl method body (spec 0020) with `Self` bound to the target
    /// and the impl's bounds in scope for calls on its type parameters.
    fn check_impl_method(&self, decl: &ImplDecl, method: &Function) -> Result<()> {
        let mut subst = HashMap::new();
        subst.insert("Self".to_string(), decl.target.clone());
        let mut scope = HashMap::new();
        for param in &method.params {
            let ty = subst_type(&param.ty, &subst);
            self.record(
                &param.name_span,
                &ty,
                EntryKind::Binding(param.name.clone()),
            );
            scope.insert(param.name.clone(), ty);
        }
        let ret = subst_type(&method.ret, &subst);
        let throws = method.throws.as_ref().map(|t| subst_type(t, &subst));
        self.check_effect_row(&method.effects, &method.name_span)?;
        let bounds = bounds_map(&decl.bounds);
        let ctx = FnCtx {
            throws: &throws,
            bounds: &bounds,
            // Impl methods resolve bare names from their own module's scope
            // (spec 0037): the qualifier their module's functions carry, or the
            // compilation root for a root-file impl.
            module: &method.module_path,
            effects: &method.effects,
            declared_module: method.declared_module.as_deref(),
            effect_name: None,
            implicit_try: false,
        };
        let body = self.check_block(&method.body, &mut scope, &ctx, false, Some(&ret))?;
        expect_assignable(&body.ty, &ret, body.span.clone())?;
        if !body.effects.is_subset_of(&method.effects) {
            return Err(Error::diagnostic(
                Diagnostic::new("Unhandled effects").label(
                    body.span.clone(),
                    format!(
                        "method `{}` declares uses {:?}, but body uses {:?}",
                        method.name, method.effects.effects, body.effects.effects
                    ),
                ),
            ));
        }
        self.check_throws_subset(&body.throws, &throws, &method.name, body.span)?;
        Ok(())
    }

    /// The module that owns `ty` for the orphan rule (spec 0020). Built-in types
    /// are owned by Core Prelude (spec 0021). `None` means the type has no
    /// nameable owner (e.g. a function type).
    fn type_owning_module(&self, ty: &Type) -> Option<Option<String>> {
        match ty {
            // A record and an enum share the `Type::Enum(name, _)` representation
            // (nominal), but live in separate tables. Consult both so an `impl` on
            // a user record (spec 0006) — generic or not — resolves its owning
            // module for the orphan rule, just like an enum.
            Type::Enum(name, _) => self
                .enums
                .get(name)
                .map(|info| info.module.clone())
                .or_else(|| self.records.get(name).map(|info| info.module.clone())),
            Type::Int
            | Type::Float
            | Type::String
            | Type::Char
            | Type::Bytes
            | Type::Bool
            | Type::Unit
            | Type::Array(_) => Some(Some(crate::prelude::CORE_MODULE.to_string())),
            _ => None,
        }
    }

    /// Resolves a trait method call from the argument types (spec 0020): infers
    /// `Self`, discharges the bound (bounded type parameter or concrete impl),
    /// and returns the result type/effects/throws.
    #[allow(clippy::too_many_arguments)]
    fn dispatch_method(
        &self,
        candidates: &[String],
        method_name: &str,
        args: &[ExprInfo],
        span: &Span,
        ctx: &FnCtx,
        allow_throw: bool,
        expected: Option<&Type>,
    ) -> Result<ExprInfo> {
        if candidates.len() > 1 {
            return Err(ambiguous_method_error(method_name, candidates, span));
        }
        let trait_name = &candidates[0];
        let Some(trait_info) = self.traits.get(trait_name) else {
            return Err(Error::diagnostic(Diagnostic::new("Unknown trait").label(
                span.clone(),
                format!("`{trait_name}` is not a declared trait"),
            )));
        };
        let Some(tmethod) = trait_info.methods.iter().find(|m| m.name == method_name) else {
            return Err(Error::diagnostic(Diagnostic::new("Unknown method").label(
                span.clone(),
                format!("`{trait_name}` has no method `{method_name}`"),
            )));
        };
        if args.len() != tmethod.params.len() {
            return Err(Error::diagnostic(
                Diagnostic::new("Wrong number of arguments").label(
                    span.clone(),
                    format!(
                        "`{trait_name}.{method_name}` takes {} argument(s), got {}",
                        tmethod.params.len(),
                        args.len()
                    ),
                ),
            ));
        }
        let mut subst = HashMap::new();
        for (declared, arg) in tmethod.params.iter().zip(args.iter()) {
            match_type(declared, &arg.ty, &mut subst, &arg.span)?;
        }
        // Return dispatch (spec 0047): when `Self` is not pinned by an argument
        // (e.g. `empty()`), resolve it from the call's expected type by unifying
        // the declared return type against it. `Never` (an unresolved hole, spec
        // 0028) carries no information, so it does not resolve `Self`.
        if !subst.contains_key("Self")
            && tmethod.return_dispatch
            && let Some(exp) = expected
            && !matches!(exp, Type::Never)
        {
            let _ = match_type(&tmethod.ret, exp, &mut subst, span);
        }
        let Some(self_ty) = subst.get("Self").cloned() else {
            return Err(Error::diagnostic(
                Diagnostic::new("Cannot infer Self")
                    .label(
                        span.clone(),
                        format!(
                            "could not determine the `Self` type of `{trait_name}.{method_name}`"
                        ),
                    )
                    .help(
                        "Add a type annotation so the expected type is known, \
                         e.g. `let x: T = ...` (spec 0047).",
                    ),
            ));
        };
        self.check_bound_satisfied(trait_name, &self_ty, ctx, span)?;
        let mut effects = tmethod.effects.clone();
        let mut throws = None;
        for arg in args {
            effects.union(&arg.effects);
            throws = merge_throws(throws, arg.throws.clone(), arg.span.clone())?;
        }
        if let Some(err) = &tmethod.throws {
            if allow_throw {
                throws = merge_throws(throws, Some(subst_type(err, &subst)), span.clone())?;
            } else if !ctx.implicit_try {
                return Err(unhandled_throwing_call(span));
            }
        }
        Ok(ExprInfo {
            ty: subst_type(&tmethod.ret, &subst),
            effects,
            throws,
            span: span.clone(),
        })
    }

    /// Checks that `ty` satisfies `trait_name` (spec 0020): a still-abstract type
    /// parameter is fine if the enclosing definition already bounds it by the
    /// trait (bound propagation); otherwise the concrete type needs an impl.
    fn check_bound_satisfied(
        &self,
        trait_name: &str,
        ty: &Type,
        ctx: &FnCtx,
        span: &Span,
    ) -> Result<()> {
        match ty {
            Type::Var(v)
                if ctx
                    .bounds
                    .get(v)
                    .is_some_and(|ts| ts.iter().any(|t| t == trait_name)) =>
            {
                Ok(())
            }
            Type::Var(v) => Err(unsatisfied_bound_error(v, trait_name, span)),
            concrete => self.discharge(trait_name, concrete, span),
        }
    }

    /// Confirms a concrete type satisfies a trait (spec 0020): finds the unique
    /// impl by type head and recursively discharges that impl's own bounds.
    fn discharge(&self, trait_name: &str, concrete: &Type, span: &Span) -> Result<()> {
        let Some(key) = type_head_key(concrete) else {
            return Err(unsatisfied_bound_error_ty(concrete, trait_name, span));
        };
        let Some(&idx) = self.impls_by.get(&(trait_name.to_string(), key)) else {
            return Err(unsatisfied_bound_error_ty(concrete, trait_name, span));
        };
        let impl_info = &self.impls[idx];
        let mut isubst = HashMap::new();
        match_type(&impl_info.target, concrete, &mut isubst, span)?;
        for bound in &impl_info.bounds {
            let arg = isubst
                .get(&bound.param)
                .cloned()
                .unwrap_or_else(|| Type::Var(bound.param.clone()));
            for tr in &bound.traits {
                self.discharge(tr, &arg, span)?;
            }
        }
        Ok(())
    }

    fn register_functions(&mut self, program: &Program, errors: &mut Vec<Error>) {
        // Imported public functions carry a qualifier (spec 0018) and may share a
        // bare name with one another (resolved by qualifying at the call site).
        // Only unqualified functions — the compilation root's own functions and
        // module-private helpers — still need a unique bare name, since they
        // share a backend emit name.
        let mut seen_local = HashSet::new();
        for function in &program.functions {
            if let Err(error) = self.validate_function_decl(function, &mut seen_local) {
                errors.push(error);
            }
            // The signature is recorded even when validation failed: `sigs`
            // must stay index-parallel with `Program::functions` so `FnTable`
            // entries keep resolving (spec 0033).
            self.sigs.push(FunctionSig {
                type_params: function.type_params.clone(),
                bounds: function.bounds.clone(),
                params: function
                    .params
                    .iter()
                    .map(|param| param.ty.clone())
                    .collect(),
                ret: function.ret.clone(),
                throws: function.throws.clone(),
                // An effect operation's caller-facing signature contributes the
                // *effect* `Name` (a capability marker), not its dependency row
                // (spec 0049 D1): `Log.info(...)` makes the caller `uses { Log }`
                // even though `info`'s body depends on `Io`. `function.effects`
                // (the dependencies) is what `check_function` checks the body
                // against; the signature is what call sites union in. Ordinary
                // functions contribute their own effects unchanged.
                effects: match &function.effect_name {
                    Some(effect) => EffectRow::sorted(vec![effect.clone()]),
                    None => function.effects.clone(),
                },
            });
        }
    }

    /// The per-declaration validations of `register_functions`, split out so a
    /// failure is reported without losing the function's table slot.
    fn validate_function_decl(
        &self,
        function: &Function,
        seen_local: &mut HashSet<String>,
    ) -> Result<()> {
        if function.module_path.is_empty() && !seen_local.insert(function.name.clone()) {
            return Err(Error::diagnostic(
                Diagnostic::new("Duplicate function").label(
                    function.name_span.clone(),
                    format!("function `{}` is already defined", function.name),
                ),
            ));
        }
        let mut names = HashSet::new();
        for param in &function.params {
            self.validate_type(&param.ty, &param.name_span)?;
            if !names.insert(param.name.clone()) {
                return Err(Error::diagnostic(
                    Diagnostic::new("Duplicate parameter").label(
                        param.name_span.clone(),
                        format!("parameter `{}` is already defined", param.name),
                    ),
                ));
            }
        }
        self.validate_type(&function.ret, &function.name_span)?;
        if let Some(throws) = &function.throws {
            self.validate_type(throws, &function.name_span)?;
        }
        // Every type parameter must occur in at least one parameter type
        // (possibly nested), so a call can infer it from its arguments
        // (spec 0014). Type arguments are not given explicitly.
        let mut mentioned = HashSet::new();
        for param in &function.params {
            collect_type_vars(&param.ty, &mut mentioned);
        }
        for type_param in &function.type_params {
            if !mentioned.contains(type_param) {
                return Err(Error::diagnostic(
                        Diagnostic::new("Uninferable type parameter")
                            .label(
                                function.name_span.clone(),
                                format!(
                                    "type parameter `{type_param}` does not appear in any parameter type"
                                ),
                            )
                            .help(
                                "Each type parameter must be inferable from an argument; \
                                 use it in a parameter type.",
                            ),
                    ));
            }
        }
        // Every bound (spec 0020) must name one of this function's type
        // parameters and a declared trait.
        for bound in &function.bounds {
            if !function.type_params.contains(&bound.param) {
                return Err(Error::diagnostic(
                    Diagnostic::new("Unknown type parameter").label(
                        bound.span.clone(),
                        format!(
                            "`{}` is not a type parameter of `{}`",
                            bound.param, function.name
                        ),
                    ),
                ));
            }
            for tr in &bound.traits {
                if !self.traits.contains_key(tr) {
                    return Err(Error::diagnostic(Diagnostic::new("Unknown trait").label(
                        bound.span.clone(),
                        format!("`{tr}` is not a declared trait"),
                    )));
                }
            }
        }
        Ok(())
    }

    /// Validates each `extern fn` against the platform interface (spec 0013) and
    /// registers it as a callable signature so wrappers can call it. Each
    /// declaration is checked independently (spec 0033): a broken one is
    /// reported and skipped.
    fn register_externs(&mut self, program: &Program, errors: &mut Vec<Error>) {
        for declaration in &program.externs {
            if let Err(error) = self.register_extern(declaration) {
                errors.push(error);
            }
        }
    }

    fn register_extern(&mut self, declaration: &Extern) -> Result<()> {
        let clashes_function = !matches!(
            self.table.resolve(std::slice::from_ref(&declaration.name)),
            Resolved::None
        );
        let params: Vec<Type> = declaration
            .params
            .iter()
            .map(|param| param.ty.clone())
            .collect();
        // An `intrinsic fn` (spec 0021) validates against the intrinsic
        // interface, must be pure, and registers like a callable so wrappers
        // can call it. Lowering turns the call into an Intrinsic node.
        if declaration.is_intrinsic {
            let Some(entry) = emela_codegen::intrinsic_lookup(&declaration.name) else {
                return Err(Error::diagnostic(
                    Diagnostic::new("Unknown intrinsic")
                        .label(
                            declaration.name_span.clone(),
                            format!("`{}` is not an intrinsic", declaration.name),
                        )
                        .help("Intrinsics are defined by spec 0021."),
                ));
            };
            // The signature must match the interface exactly, type parameters
            // included. A generic intrinsic (spec 0021) is written over the same
            // type-variable names as the interface entry (`T`), so the `params`/
            // `ret` comparison — which contains `Type::Var("T")` on both sides —
            // and the `type_params` comparison together pin the shape.
            if params != entry.params
                || declaration.ret != entry.ret
                || declaration.type_params != entry.type_params
            {
                return Err(Error::diagnostic(
                    Diagnostic::new("Intrinsic signature mismatch").label(
                        declaration.name_span.clone(),
                        format!(
                            "`{}` does not match the intrinsic interface",
                            declaration.name
                        ),
                    ),
                ));
            }
            if !declaration.effects.effects.is_empty() || declaration.throws.is_some() {
                return Err(Error::diagnostic(
                    Diagnostic::new("Intrinsic must be pure").label(
                        declaration.name_span.clone(),
                        format!(
                            "`{}` must declare `uses {{}}` and no `throws`",
                            declaration.name
                        ),
                    ),
                ));
            }
            // Every intrinsic is declared exactly once, in the embedded std
            // (spec 0038); user-source declarations are rejected and dropped
            // before registration (`reject_user_intrinsics`), so a repeat
            // here is a genuine duplicate.
            if self.externs.contains_key(&declaration.name) || clashes_function {
                return Err(duplicate_function_error(declaration));
            }
            self.externs.insert(
                declaration.name.clone(),
                ExternSig {
                    sig: FunctionSig {
                        // A generic intrinsic (spec 0021) carries its type
                        // parameters so a call is monomorphized like a generic
                        // function (spec 0014). Empty for a non-generic one.
                        type_params: declaration.type_params.clone(),
                        bounds: declaration.bounds.clone(),
                        params,
                        ret: declaration.ret.clone(),
                        throws: None,
                        effects: declaration.effects.clone(),
                    },
                    module: declaration.module.clone(),
                    effect_name: declaration.effect_name.clone(),
                    is_intrinsic: true,
                },
            );
            return Ok(());
        }
        // A platform function must not collide with anything already defined.
        if self.externs.contains_key(&declaration.name) || clashes_function {
            return Err(duplicate_function_error(declaration));
        }
        let canonical = declaration.canonical();
        let Some(entry) = emela_codegen::platform_lookup_in(self.platform_registry, &canonical)
        else {
            return Err(Error::diagnostic(
                Diagnostic::new("Unknown platform function")
                    .label(
                        declaration.name_span.clone(),
                        format!("`{canonical}` is not a platform function"),
                    )
                    .help("Platform functions are defined by spec 0013."),
            ));
        };
        if params != entry.params
            || declaration.ret != entry.ret
            || declaration.throws != entry.throws
        {
            // The `throws` clause is part of the registry signature (spec 0043).
            return Err(Error::diagnostic(
                Diagnostic::new("Platform signature mismatch").label(
                    declaration.name_span.clone(),
                    format!("`{canonical}` does not match the platform interface"),
                ),
            ));
        }
        let expected = EffectRow::sorted(vec![entry.capability.clone()]);
        if declaration.effects != expected {
            return Err(Error::diagnostic(
                Diagnostic::new("Platform effect mismatch").label(
                    declaration.name_span.clone(),
                    format!(
                        "`{canonical}` must declare `uses {{ {} }}`",
                        entry.capability
                    ),
                ),
            ));
        }
        self.externs.insert(
            declaration.name.clone(),
            ExternSig {
                sig: FunctionSig {
                    // Platform functions are never generic (spec 0013).
                    type_params: Vec::new(),
                    bounds: Vec::new(),
                    params,
                    ret: declaration.ret.clone(),
                    throws: declaration.throws.clone(),
                    effects: declaration.effects.clone(),
                },
                module: declaration.module.clone(),
                effect_name: declaration.effect_name.clone(),
                is_intrinsic: false,
            },
        );
        Ok(())
    }

    /// The extern/intrinsic signature `name` resolves to from the current
    /// context (spec 0037): an effect backing operation is visible only to
    /// sibling operations of its effect; any other extern only within its
    /// declaring module. Everywhere else the name does not exist — externs
    /// never cross a module boundary by bare name.
    fn visible_extern(&self, name: &str, ctx: &FnCtx) -> Option<&FunctionSig> {
        let entry = self.externs.get(name)?;
        // A Core Prelude intrinsic (spec 0021) is bare-visible from every module:
        // the prelude is implicitly imported everywhere, so its pure primitives
        // (e.g. `char_from_code`, `array_push`) cross module boundaries by bare
        // name — unlike a module-private extern or a prelude-external intrinsic.
        if entry.is_intrinsic && entry.module.as_deref() == Some(crate::prelude::CORE_MODULE) {
            return Some(&entry.sig);
        }
        let visible = match &entry.effect_name {
            Some(effect) => ctx.effect_name == Some(effect.as_str()),
            None => {
                ctx.declared_module == entry.module.as_deref()
                    // Host interface externs (spec 0026) are callable from any
                    // module that imports their host interface package.
                    || entry
                        .module
                        .as_deref()
                        .is_some_and(|m| m.starts_with("host."))
                        && !entry.is_intrinsic
            }
        };
        visible.then_some(&entry.sig)
    }

    /// The `uses` gate (spec 0037): an effect-operation reference resolves only
    /// when the lexically enclosing function/literal declares the effect in its
    /// `uses` row. The row-subset check (spec 0023) still runs as the semantic
    /// backstop; this gate exists to fail at the reference, naming the effect.
    fn check_effect_gate(&self, entry: &FnEntry, ctx: &FnCtx, span: &Span) -> Result<()> {
        let Some(effect) = &entry.effect_name else {
            return Ok(());
        };
        if ctx
            .effects
            .effects
            .iter()
            .any(|declared| declared == effect)
        {
            return Ok(());
        }
        Err(Error::diagnostic(
            Diagnostic::new("Effect not declared in `uses`")
                .label(
                    span.clone(),
                    format!(
                        "effect `{effect}` is not declared in the enclosing function's `uses` clause"
                    ),
                )
                .help(format!(
                    "Operations of `{effect}` are usable only inside a `uses {{ {effect} }}` scope (spec 0037)."
                )),
        ))
    }

    /// Every name in a declared `uses {{ ... }}` row must resolve to an effect
    /// in scope (spec 0037): declared in this file or brought in by an import.
    fn check_effect_row(&self, row: &EffectRow, span: &Span) -> Result<()> {
        for name in &row.effects {
            if name == "host" {
                return Err(Error::diagnostic(
                    Diagnostic::new("Bare host capability")
                        .label(
                            span.clone(),
                            "`host` is not a standalone capability (spec 0026)".to_string(),
                        )
                        .help("Use `host.<name>` instead, e.g. `host.gpio`."),
                ));
            }
            if self.effects_in_scope.contains(name) {
                continue;
            }
            let mut diagnostic = Diagnostic::new("Unknown effect")
                .label(span.clone(), format!("`{name}` is not an effect in scope"));
            if let Some(hint) = effect_scope_hint(name, &self.effects_in_scope) {
                diagnostic = diagnostic.help(hint);
            }
            return Err(Error::diagnostic(diagnostic));
        }
        Ok(())
    }

    /// The diagnostic for a dotted path that resolved to nothing, layered by
    /// cause (spec 0037): an enum head means the old `.` variant spelling, a
    /// known effect head means a missing/private operation, an unknown
    /// capitalized head is probably an effect that was never imported.
    fn unknown_path_error(&self, segments: &[String], span: &Span) -> Error {
        let path = segments.join(".");
        if segments.len() == 2 && self.enums.contains_key(&segments[0]) {
            // A dotted path whose head is a declared enum is almost certainly a
            // variant written with the old `.` spelling; point the user at the
            // `::` type path (spec 0018 R7).
            return Error::diagnostic(Diagnostic::new("Unknown name").label(
                span.clone(),
                format!(
                    "enum variants use `::`: write `{0}::{1}`, not `{0}.{1}`",
                    segments[0], segments[1]
                ),
            ));
        }
        if segments.len() == 2 && self.effects_in_scope.contains(&segments[0]) {
            let (effect, op) = (&segments[0], &segments[1]);
            // A backing extern is a private operation of its effect (spec 0037).
            if let Some(entry) = self.externs.get(op)
                && entry.effect_name.as_deref() == Some(effect.as_str())
            {
                return Error::diagnostic(Diagnostic::new("Private effect operation").label(
                    span.clone(),
                    format!("`{op}` is a private operation of effect `{effect}`"),
                ));
            }
            return Error::diagnostic(Diagnostic::new("Unknown name").label(
                span.clone(),
                format!("effect `{effect}` has no public operation `{op}`"),
            ));
        }
        // A capitalized head that is not a trait, enum, or in-scope effect is
        // probably an effect whose module was never imported (spec 0037).
        if segments.len() == 2
            && segments[0].chars().next().is_some_and(char::is_uppercase)
            && !self.traits.contains_key(&segments[0])
        {
            let mut diagnostic = Diagnostic::new("Unknown effect").label(
                span.clone(),
                format!("`{}` is not an effect in scope", segments[0]),
            );
            if let Some(hint) = effect_scope_hint(&segments[0], &self.effects_in_scope) {
                diagnostic = diagnostic.help(hint);
            }
            return Error::diagnostic(diagnostic);
        }
        Error::diagnostic(
            Diagnostic::new("Unknown name").label(span.clone(), format!("`{path}` is not defined")),
        )
    }

    fn check_main(&self, program: &Program) -> Result<()> {
        let Some(main) = program
            .functions
            .iter()
            .find(|function| function.name == "main")
        else {
            let span = program
                .functions
                .first()
                .map(|function| function.name_span.clone())
                .ok_or_else(|| Error::new("program has no functions"))?;
            return Err(Error::diagnostic(
                Diagnostic::new("Missing entrypoint")
                    .label(span, "expected a top-level `main` function"),
            ));
        };
        if !main.params.is_empty() {
            return Err(Error::diagnostic(
                Diagnostic::new("Invalid entrypoint")
                    .label(main.name_span.clone(), "`main` must not take parameters"),
            ));
        }
        // The entrypoint's throws channel must be `Never`, i.e. non-throwing
        // (spec 0011). `throws Never` is normalized to `None` by the parser, so
        // any remaining declared error type is rejected here.
        if main.throws.is_some() {
            return Err(Error::diagnostic(
                Diagnostic::new("Invalid entrypoint")
                    .label(main.name_span.clone(), "`main`'s `throws` must be `Never`")
                    .help(
                        "Omit `throws` (or write `throws Never`); handle errors with `try`/`catch` or `panic`.",
                    ),
            ));
        }
        Ok(())
    }

    /// Checks one top-level function and returns the effect row its body
    /// actually requires (always a subset of the declared row); the caller
    /// records it on the `TypedFunction` for the over-declared-effects lint.
    fn check_function(&self, function: &Function) -> Result<EffectRow> {
        // The declared `uses` row must name effects that exist (spec 0037).
        self.check_effect_row(&function.effects, &function.name_span)?;
        let mut scope = HashMap::new();
        for param in &function.params {
            self.record(
                &param.name_span,
                &param.ty,
                EntryKind::Binding(param.name.clone()),
            );
            scope.insert(param.name.clone(), param.ty.clone());
        }
        let bounds = bounds_map(&function.bounds);
        let ctx = FnCtx {
            throws: &function.throws,
            bounds: &bounds,
            module: &function.module_path,
            effects: &function.effects,
            declared_module: function.declared_module.as_deref(),
            effect_name: function.effect_name.as_deref(),
            implicit_try: function.is_test,
        };
        let body =
            self.check_block(&function.body, &mut scope, &ctx, false, Some(&function.ret))?;
        expect_assignable(&body.ty, &function.ret, body.span.clone())?;
        if !body.effects.is_subset_of(&function.effects) {
            return Err(Error::diagnostic(
                Diagnostic::new("Unhandled effects")
                    .label(
                        body.span.clone(),
                        format!(
                            "function `{}` declares uses {:?}, but body uses {:?}",
                            function.name, function.effects.effects, body.effects.effects
                        ),
                    )
                    .help("Add the missing effect names to `uses { ... }`."),
            ));
        }
        self.check_throws_subset(&body.throws, &function.throws, &function.name, body.span)?;
        Ok(body.effects)
    }

    /// The body may only put on the throws channel what the function declares.
    fn check_throws_subset(
        &self,
        body: &Option<Type>,
        declared: &Option<Type>,
        name: &str,
        span: Span,
    ) -> Result<()> {
        match (body, declared) {
            (None, _) => Ok(()),
            (Some(actual), Some(expected)) if types_compatible(actual, expected) => Ok(()),
            (Some(actual), Some(expected)) => Err(Error::diagnostic(
                Diagnostic::new("Throws type mismatch").label(
                    span,
                    format!(
                        "`{name}` declares `throws {expected:?}`, but the body throws `{actual:?}`"
                    ),
                ),
            )),
            (Some(actual), None) => Err(Error::diagnostic(
                Diagnostic::new("Unhandled error")
                    .label(
                        span,
                        format!("`{name}` may throw `{actual:?}`, but declares no `throws`"),
                    )
                    .help("Add a `throws E` clause, or handle the error with `try`/`catch`."),
            )),
        }
    }

    fn check_block(
        &self,
        block: &Block,
        outer_scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
        allow_throw: bool,
        expected: Option<&Type>,
    ) -> Result<ExprInfo> {
        let mut scope = outer_scope.clone();
        let mut effects = EffectRow::default();
        let mut throws: Option<Type> = None;
        // The block's value is its tail expression, so only the tail inherits the
        // block's expected type (spec 0047); earlier statements do not.
        let last_ix = block.items.len().saturating_sub(1);
        let mut last = ExprInfo {
            ty: Type::Unit,
            effects: EffectRow::default(),
            throws: None,
            span: block.span.clone(),
        };
        for (ix, item) in block.items.iter().enumerate() {
            match item {
                BlockItem::Let {
                    name,
                    name_span,
                    ty,
                    value,
                } => {
                    if scope.contains_key(name) {
                        return Err(Error::diagnostic(
                            Diagnostic::new("Duplicate binding")
                                .label(name_span.clone(), format!("`{name}` is already bound")),
                        ));
                    }
                    let info = match (value, ty) {
                        (Expr::Array(elements, span), Some(Type::Array(element))) => {
                            // Checked directly, not via `check_expr_expected`,
                            // so the index entry is recorded here.
                            let info = self.check_array(
                                elements,
                                span,
                                &mut scope,
                                ctx,
                                Some(element),
                                allow_throw,
                            )?;
                            self.record(&info.span, &info.ty, EntryKind::Expr);
                            info
                        }
                        // An annotated `let` gives its value an expected type, so
                        // a return-dispatched `empty()` resolves here (spec 0047).
                        (_, Some(annotation)) => self.check_expr_expected(
                            value,
                            &mut scope,
                            ctx,
                            allow_throw,
                            Some(annotation),
                        )?,
                        _ => self.check_expr(value, &mut scope, ctx, allow_throw)?,
                    };
                    let binding_ty = if let Some(annotation) = ty {
                        expect_assignable(&info.ty, annotation, info.span.clone())?;
                        annotation.clone()
                    } else {
                        info.ty
                    };
                    // The binding's type — the annotation, or the inferred
                    // value type — is what hover shows for the name.
                    self.record(name_span, &binding_ty, EntryKind::Binding(name.clone()));
                    effects.union(&info.effects);
                    throws = merge_throws(throws, info.throws, info.span)?;
                    scope.insert(name.clone(), binding_ty);
                    last = ExprInfo {
                        ty: Type::Unit,
                        effects: EffectRow::default(),
                        throws: None,
                        span: name_span.clone(),
                    };
                }
                BlockItem::Expr(expr) => {
                    let item_expected = if ix == last_ix { expected } else { None };
                    last = self.check_expr_expected(
                        expr,
                        &mut scope,
                        ctx,
                        allow_throw,
                        item_expected,
                    )?;
                    effects.union(&last.effects);
                    throws = merge_throws(throws, last.throws.clone(), last.span.clone())?;
                }
            }
        }
        last.effects = effects;
        last.throws = throws;
        Ok(last)
    }

    fn check_expr(
        &self,
        expr: &Expr,
        scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
        allow_throw: bool,
    ) -> Result<ExprInfo> {
        self.check_expr_expected(expr, scope, ctx, allow_throw, None)
    }

    /// Like `check_expr` but in checking mode (spec 0047): `expected` is the type
    /// the surrounding context wants here, used to resolve return-dispatched
    /// trait methods (`empty()`). It is threaded only through positions that
    /// propagate an expected type (block tail, `let` annotation, call argument,
    /// `if`/`match` branches); elsewhere it is `None`.
    ///
    /// Every expression flows through here, so this wrapper is the single
    /// place the type index (spec 0033) records expression types.
    fn check_expr_expected(
        &self,
        expr: &Expr,
        scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
        allow_throw: bool,
        expected: Option<&Type>,
    ) -> Result<ExprInfo> {
        let info = self.check_expr_expected_inner(expr, scope, ctx, allow_throw, expected)?;
        self.record(&info.span, &info.ty, EntryKind::Expr);
        Ok(info)
    }

    fn check_expr_expected_inner(
        &self,
        expr: &Expr,
        scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
        allow_throw: bool,
        expected: Option<&Type>,
    ) -> Result<ExprInfo> {
        match expr {
            Expr::Int(_, span) => Ok(self.info(Type::Int, span.clone())),
            Expr::Float(_, span) => Ok(self.info(Type::Float, span.clone())),
            Expr::Bool(_, span) => Ok(self.info(Type::Bool, span.clone())),
            Expr::String(_, span) => Ok(self.info(Type::String, span.clone())),
            Expr::Char(_, span) => Ok(self.info(Type::Char, span.clone())),
            Expr::Array(elements, span) => {
                self.check_array(elements, span, scope, ctx, None, allow_throw)
            }
            Expr::RecordLiteral {
                name,
                name_span,
                fields,
                span,
            } => self.check_record_literal(name, name_span, fields, span, scope, ctx, allow_throw),
            Expr::Field {
                target,
                name,
                name_span,
                span,
            } => {
                let info = self.check_expr(target, scope, ctx, allow_throw)?;
                let ty = self.field_type(&info.ty, name, name_span)?;
                Ok(ExprInfo {
                    ty,
                    effects: info.effects,
                    throws: info.throws,
                    span: span.clone(),
                })
            }
            Expr::Unit(span) => Ok(self.info(Type::Unit, span.clone())),
            Expr::Var(name, span) => {
                if let Some(ty) = scope.get(name) {
                    Ok(self.info(ty.clone(), span.clone()))
                } else if let Some(li) = &self.option_lang_item
                    && *name == li.none
                {
                    // Bare `None` (spec 0042 O3): the absent variant of the
                    // `option` lang item, constructed like `Option::None`. Its
                    // type parameter is unpinned, so `check_variant` leaves it
                    // `Never` (`None : Option<Never>`).
                    let segments = [li.enum_name.clone(), name.clone()];
                    self.check_variant(&segments, &[], span, scope, ctx, allow_throw)
                } else if let Some(sig) = self.visible_extern(name, ctx) {
                    // A generic intrinsic (spec 0021), like a generic function,
                    // fixes its type arguments only at a call site; it has no
                    // first-class function type to flow through here.
                    if sig.is_generic() {
                        return Err(generic_value_error(name, span));
                    }
                    Ok(self.info(sig.ty(), span.clone()))
                } else {
                    match self
                        .table
                        .resolve_in(std::slice::from_ref(name), ctx.module)
                    {
                        Resolved::One(entry) => {
                            self.check_effect_gate(entry, ctx, span)?;
                            let sig = &self.sigs[entry.index];
                            // A generic function cannot be used as a first-class
                            // value: its type arguments are only fixed at a direct
                            // call site (spec 0014).
                            if sig.is_generic() {
                                return Err(generic_value_error(name, span));
                            }
                            Ok(self.info(sig.ty(), span.clone()))
                        }
                        Resolved::Ambiguous(candidates) => {
                            Err(ambiguous_error(name, &candidates, span))
                        }
                        Resolved::EffectOpUnqualified(entry) => {
                            Err(effect_op_unqualified_error(name, entry, span))
                        }
                        Resolved::BareImported(candidates) => {
                            Err(bare_imported_error(name, &candidates, span))
                        }
                        Resolved::Private(candidates) => {
                            Err(private_reference_error(name, &candidates, span))
                        }
                        Resolved::None => {
                            Err(Error::diagnostic(Diagnostic::new("Unknown name").label(
                                span.clone(),
                                format!("`{name}` is not defined in this scope"),
                            )))
                        }
                    }
                }
            }
            Expr::Call { callee, args, span } => {
                self.check_call(callee, args, span, scope, ctx, allow_throw, expected)
            }
            Expr::Fn {
                params,
                ret,
                throws,
                effects,
                body,
                span,
            } => {
                let mut names = HashSet::new();
                let mut fn_scope = scope.clone();
                for param in params {
                    self.validate_type(&param.ty, &param.name_span)?;
                    if !names.insert(param.name.clone()) {
                        return Err(Error::diagnostic(
                            Diagnostic::new("Duplicate parameter").label(
                                param.name_span.clone(),
                                format!("parameter `{}` is already defined", param.name),
                            ),
                        ));
                    }
                    self.record(
                        &param.name_span,
                        &param.ty,
                        EntryKind::Binding(param.name.clone()),
                    );
                    fn_scope.insert(param.name.clone(), param.ty.clone());
                }
                // The literal's own declared row is the `uses` gate inside its
                // body (spec 0037); everything else is inherited lexically.
                self.check_effect_row(effects, span)?;
                let inner_ctx = FnCtx {
                    throws,
                    bounds: ctx.bounds,
                    module: ctx.module,
                    effects,
                    declared_module: ctx.declared_module,
                    effect_name: ctx.effect_name,
                    // A nested literal's body follows the ordinary rules even
                    // inside a `@test` body (spec 0040 T3).
                    implicit_try: false,
                };
                let body_info =
                    self.check_block(body, &mut fn_scope, &inner_ctx, false, Some(ret))?;
                expect_assignable(&body_info.ty, ret, body_info.span.clone())?;
                if !body_info.effects.is_subset_of(effects) {
                    return Err(Error::diagnostic(
                        Diagnostic::new("Unhandled effects")
                            .label(
                                body_info.span.clone(),
                                format!(
                                    "function literal declares uses {:?}, but body uses {:?}",
                                    effects.effects, body_info.effects.effects
                                ),
                            )
                            .help("Add the missing effect names to `uses { ... }`."),
                    ));
                }
                self.check_throws_subset(
                    &body_info.throws,
                    throws,
                    "function literal",
                    body_info.span,
                )?;
                Ok(ExprInfo {
                    ty: Type::Function(FunctionType {
                        params: params.iter().map(|param| param.ty.clone()).collect(),
                        ret: Box::new(ret.clone()),
                        throws: throws.clone().map(Box::new),
                        effects: effects.clone(),
                    }),
                    effects: EffectRow::default(),
                    throws: None,
                    span: span.clone(),
                })
            }
            Expr::Binary {
                op,
                left,
                right,
                span,
            } => {
                let left = self.check_expr(left, scope, ctx, allow_throw)?;
                let right = self.check_expr(right, scope, ctx, allow_throw)?;
                // Every operator desugars to a trait method call on its operands
                // (spec 0020): `a + b` is `Add.add(a, b)`. The built-in numeric and
                // string instances come from the Core Prelude (spec 0021), so the
                // compiler holds no operator-specific type rules.
                let (trait_name, method) = operator_trait(*op);
                let candidates = [trait_name.to_string()];
                self.dispatch_method(
                    &candidates,
                    method,
                    &[left, right],
                    span,
                    ctx,
                    allow_throw,
                    None,
                )
            }
            Expr::Block(block) => self.check_block(block, scope, ctx, allow_throw, expected),
            Expr::Throw { value, span } => {
                let val = self.check_expr(value, scope, ctx, allow_throw)?;
                // In a `@test` body a `throw` outside `try` fails the test at
                // this site (spec 0040 T3) instead of using the throws channel.
                let throws = if ctx.implicit_try && !allow_throw {
                    None
                } else {
                    Some(val.ty)
                };
                Ok(ExprInfo {
                    ty: Type::Never,
                    effects: val.effects,
                    throws,
                    span: span.clone(),
                })
            }
            Expr::Panic { message, span } => {
                let message = self.check_expr(message, scope, ctx, allow_throw)?;
                expect_assignable(&message.ty, &Type::String, message.span.clone())?;
                Ok(ExprInfo {
                    ty: Type::Never,
                    effects: message.effects,
                    throws: None,
                    span: span.clone(),
                })
            }
            Expr::Question { value, span } => {
                let inner = self.check_expr(value, scope, ctx, true)?;
                if let Some(error) = inner.throws.clone() {
                    // Throws propagation: `?` forwards the error to the
                    // enclosing function's throws channel (spec 0011).
                    match ctx.throws {
                        Some(declared) if types_compatible(&error, declared) => {}
                        Some(declared) => {
                            return Err(Error::diagnostic(
                                Diagnostic::new("Throws type mismatch").label(
                                    span.clone(),
                                    format!(
                                        "`?` propagates `{error:?}`, but the function declares `throws {declared:?}`"
                                    ),
                                ),
                            ));
                        }
                        None if ctx.implicit_try => {
                            // Spec 0040 T3: a test declares no `throws`, so `?`
                            // has nothing to propagate to — and needs nothing.
                            return Err(Error::diagnostic(
                                Diagnostic::new("Redundant `?` in a test").label(
                                    span.clone(),
                                    "a bare throwing call in a `@test` body already propagates as a test failure; remove the `?` (spec 0040)",
                                ),
                            ));
                        }
                        None => {
                            return Err(Error::diagnostic(
                                Diagnostic::new("Cannot propagate error").label(
                                    span.clone(),
                                    "`?` requires the enclosing function to declare `throws`",
                                ),
                            ));
                        }
                    }
                    Ok(ExprInfo {
                        ty: inner.ty,
                        effects: inner.effects,
                        throws: Some(error),
                        span: span.clone(),
                    })
                } else {
                    // `?` applies only to throwing calls (spec 0011/0042). It is
                    // not defined for `Option`, which is handled with `match` or
                    // the `std.option` combinators.
                    Err(Error::diagnostic(Diagnostic::new("Invalid `?`").label(
                        span.clone(),
                        "`?` applies only to a throwing call; `Option` is handled with `match` or `std.option` (spec 0011/0042)",
                    )))
                }
            }
            Expr::TypePath { segments, span } => {
                // A `::` type path used as a value (no `(...)`): a no-payload
                // enum variant (specs 0005/0018 R7). `::` is enum-variant-only
                // now (the former conversions are bare intrinsics, spec 0021).
                if segments.len() == 2 && self.enums.contains_key(&segments[0]) {
                    return self.check_variant(segments, &[], span, scope, ctx, allow_throw);
                }
                Err(Error::diagnostic(
                    Diagnostic::new("Unknown type path").label(
                        span.clone(),
                        format!("`{}` is not a no-payload enum variant", segments.join("::")),
                    ),
                ))
            }
            Expr::Path { segments, span } => {
                // A dotted head that is a local value is record field access
                // (spec 0006): `user.name`, `incoming.request.url`.
                if let Some(head_ty) = scope.get(&segments[0]).cloned() {
                    let mut ty = head_ty;
                    for segment in &segments[1..] {
                        ty = self.field_type(&ty, segment, span)?;
                    }
                    return Ok(self.info(ty, span.clone()));
                }
                // A dotted path used as a value (no `(...)`): an effect
                // operation (`Io.print`, spec 0037) or a module-qualified
                // function reference (`list.map`). Enum variants are `::` type
                // paths (`TypePath`), resolved separately.
                match self.table.resolve_in(segments, ctx.module) {
                    Resolved::One(entry) => {
                        self.check_effect_gate(entry, ctx, span)?;
                        let sig = &self.sigs[entry.index];
                        if sig.is_generic() {
                            return Err(generic_value_error(&segments.join("."), span));
                        }
                        Ok(self.info(sig.ty(), span.clone()))
                    }
                    Resolved::Ambiguous(candidates) => {
                        Err(ambiguous_error(&segments.join("."), &candidates, span))
                    }
                    // Unreachable for a qualified (multi-segment) path — only a
                    // bare name yields these — but handled for totality.
                    Resolved::EffectOpUnqualified(entry) => Err(effect_op_unqualified_error(
                        &segments.join("."),
                        entry,
                        span,
                    )),
                    Resolved::BareImported(candidates) => {
                        Err(bare_imported_error(&segments.join("."), &candidates, span))
                    }
                    Resolved::Private(candidates) => Err(private_reference_error(
                        &segments.join("."),
                        &candidates,
                        span,
                    )),
                    Resolved::None => Err(self.unknown_path_error(segments, span)),
                }
            }
            Expr::Match {
                scrutinee,
                arms,
                span,
            } => self.check_match(scrutinee, arms, span, scope, ctx, allow_throw, expected),
            Expr::Try { body, arms, span } => self.check_try(body, arms, span, scope, ctx),
            Expr::If {
                cond,
                then,
                els,
                span,
            } => {
                let cond_info = self.check_expr(cond, scope, ctx, allow_throw)?;
                expect_assignable(&cond_info.ty, &Type::Bool, cond_info.span.clone())?;
                let then_info = self.check_block(then, scope, ctx, allow_throw, expected)?;
                let els_info = self.check_block(els, scope, ctx, allow_throw, expected)?;
                let ty = unify_arm(Some(then_info.ty), els_info.ty, els_info.span.clone())?;
                let mut effects = cond_info.effects;
                effects.union(&then_info.effects);
                effects.union(&els_info.effects);
                let throws = merge_throws(cond_info.throws, then_info.throws, span.clone())?;
                let throws = merge_throws(throws, els_info.throws, span.clone())?;
                Ok(ExprInfo {
                    ty,
                    effects,
                    throws,
                    span: span.clone(),
                })
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn check_call(
        &self,
        callee: &Expr,
        args: &[Expr],
        span: &Span,
        scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
        allow_throw: bool,
        expected: Option<&Type>,
    ) -> Result<ExprInfo> {
        // Method-call (receiver) syntax (spec 0020): `recv.method(args)` where
        // `recv` is a value in scope desugars to `method(recv, args)`, with the
        // impl chosen from the receiver's type. This is pure sugar over the
        // free-function call, so a bare `to_string(x)` and `x.to_string()` mean
        // the same thing. A dotted head that is *not* a local value (an enum,
        // module, or trait name) keeps its qualified-path meaning.
        if let Expr::Path {
            segments,
            span: path_span,
        } = callee
            && segments.len() == 2
            && scope.contains_key(&segments[0])
        {
            let receiver = Expr::Var(segments[0].clone(), path_span.clone());
            let method = Expr::Var(segments[1].clone(), path_span.clone());
            let mut method_args = Vec::with_capacity(args.len() + 1);
            method_args.push(receiver);
            method_args.extend(args.iter().cloned());
            return self.check_call(
                &method,
                &method_args,
                span,
                scope,
                ctx,
                allow_throw,
                expected,
            );
        }
        // Bare `Some(x)` (spec 0042 O3): the present variant of the `option`
        // lang item, constructed like `Option::Some(x)`. `check_variant` checks
        // the single payload and infers the type argument.
        if let Expr::Var(name, _) = callee
            && let Some(li) = &self.option_lang_item
            && *name == li.some
            && !scope.contains_key(name)
            && self.visible_extern(name, ctx).is_none()
            && matches!(
                self.table
                    .resolve_in(std::slice::from_ref(name), ctx.module),
                Resolved::None | Resolved::BareImported(_)
            )
        {
            let segments = [li.enum_name.clone(), name.clone()];
            return self.check_variant(&segments, args, span, scope, ctx, allow_throw);
        }
        // Generic intrinsic call (spec 0021): a bare call to a generic
        // `intrinsic fn` (e.g. `array_get<T>`) infers its type argument from the
        // argument types, like a generic function. Handled before the general
        // path because a generic signature has no first-class function type to
        // flow through `check_expr` (its params contain `Type::Var`).
        if let Expr::Var(name, _) = callee
            && !scope.contains_key(name)
            && let Some(sig) = self.visible_extern(name, ctx)
            && sig.is_generic()
        {
            let sig = sig.clone();
            return self.check_generic_call(name, &sig, args, span, scope, ctx, allow_throw);
        }
        // Generic function call (spec 0014): a direct call to a generic function
        // infers its type arguments from the argument types. This is handled
        // before the general path because a generic function has no first-class
        // function type to flow through `check_expr`.
        if let Expr::Var(name, _) = callee
            && !scope.contains_key(name)
            && self.visible_extern(name, ctx).is_none()
            && let Resolved::One(entry) = self
                .table
                .resolve_in(std::slice::from_ref(name), ctx.module)
            && self.sigs[entry.index].is_generic()
        {
            self.check_effect_gate(entry, ctx, span)?;
            let sig = self.sigs[entry.index].clone();
            return self.check_generic_call(name, &sig, args, span, scope, ctx, allow_throw);
        }
        // A bare trait method call (spec 0020): a name that names a trait method
        // and is not shadowed by a binding, a visible extern, or a function of
        // the referring module. Imported functions never bind bare names (spec
        // 0037 R3), so a trait method wins over a same-named import — the Core
        // Prelude's bare names stay import-proof.
        if let Expr::Var(name, _) = callee
            && !scope.contains_key(name)
            && self.visible_extern(name, ctx).is_none()
            && matches!(
                self.table
                    .resolve_in(std::slice::from_ref(name), ctx.module),
                Resolved::None | Resolved::BareImported(_)
            )
            && let Some(candidates) = self.method_owners.get(name)
        {
            let arg_infos = args
                .iter()
                .map(|arg| self.check_expr(arg, scope, ctx, allow_throw))
                .collect::<Result<Vec<_>>>()?;
            return self.dispatch_method(
                candidates,
                name,
                &arg_infos,
                span,
                ctx,
                allow_throw,
                expected,
            );
        }
        // A qualified trait method call `Trait.method(...)` (spec 0020), used to
        // disambiguate a method name shared by several in-scope traits.
        if let Expr::Path { segments, .. } = callee
            && segments.len() == 2
            && let Some(trait_info) = self.traits.get(&segments[0])
            && trait_info.methods.iter().any(|m| m.name == segments[1])
        {
            let arg_infos = args
                .iter()
                .map(|arg| self.check_expr(arg, scope, ctx, allow_throw))
                .collect::<Result<Vec<_>>>()?;
            let candidates = [segments[0].clone()];
            return self.dispatch_method(
                &candidates,
                &segments[1],
                &arg_infos,
                span,
                ctx,
                allow_throw,
                expected,
            );
        }
        // A `::` type-path call target (specs 0005/0018 R7): an enum variant
        // constructor (`Either::Left(x)`), resolved through the enum type, never
        // through the import table. The former `Char::from_code` /
        // `String::from_char` / `Array::*` builtins are now bare intrinsics
        // (spec 0021), so `::` is enum-variant-only.
        if let Expr::TypePath {
            segments,
            span: path_span,
        } = callee
        {
            if segments.len() == 2 && self.enums.contains_key(&segments[0]) {
                return self.check_variant(segments, args, span, scope, ctx, allow_throw);
            }
            return Err(Error::diagnostic(
                Diagnostic::new("Unknown type path").label(
                    path_span.clone(),
                    format!("`{}` is not an enum variant", segments.join("::")),
                ),
            ));
        }
        // A qualified `.` call target: an effect operation (`Io.print`, spec
        // 0037) or a module-qualified function (`list.map`), possibly generic.
        // A non-generic match falls through to the general path below, where
        // `check_expr` on the path yields its function type (and applies the
        // same `uses` gate).
        if let Expr::Path { segments, .. } = callee {
            match self.table.resolve_in(segments, ctx.module) {
                Resolved::One(entry) if self.sigs[entry.index].is_generic() => {
                    self.check_effect_gate(entry, ctx, span)?;
                    let sig = self.sigs[entry.index].clone();
                    return self.check_generic_call(
                        &entry.name,
                        &sig,
                        args,
                        span,
                        scope,
                        ctx,
                        allow_throw,
                    );
                }
                Resolved::Ambiguous(candidates) => {
                    return Err(ambiguous_error(&segments.join("."), &candidates, span));
                }
                _ => {}
            }
        }
        let callee_info = self.check_expr(callee, scope, ctx, allow_throw)?;
        let Type::Function(sig) = &callee_info.ty else {
            return Err(Error::diagnostic(
                Diagnostic::new("Cannot call value").label(
                    callee_info.span.clone(),
                    format!(
                        "expected a function value, but found `{:?}`",
                        callee_info.ty
                    ),
                ),
            ));
        };
        if args.len() != sig.params.len() {
            return Err(Error::diagnostic(
                Diagnostic::new("Wrong number of arguments").label(
                    span.clone(),
                    format!(
                        "function expects {} argument(s), got {}",
                        sig.params.len(),
                        args.len()
                    ),
                ),
            ));
        }
        let mut effects = callee_info.effects.clone();
        effects.union(&sig.effects);
        let mut throws = callee_info.throws.clone();
        for (arg, param) in args.iter().zip(sig.params.iter()) {
            // The parameter type is the argument's expected type (spec 0047), so
            // a return-dispatched `empty()` in argument position resolves here.
            let actual = self.check_expr_expected(arg, scope, ctx, allow_throw, Some(param))?;
            expect_assignable(&actual.ty, param, actual.span.clone())?;
            effects.union(&actual.effects);
            throws = merge_throws(throws, actual.throws, actual.span)?;
        }
        // A throwing call must use `?` or sit inside a `try` block (spec 0011).
        // In a `@test` body the bare call instead propagates to the harness at
        // this site (spec 0040 T3) and leaves the throws channel.
        if let Some(call_error) = &sig.throws {
            if allow_throw {
                throws = merge_throws(throws, Some((**call_error).clone()), span.clone())?;
            } else if !ctx.implicit_try {
                return Err(unhandled_throwing_call(span));
            }
        }
        Ok(ExprInfo {
            ty: (*sig.ret).clone(),
            effects,
            throws,
            span: span.clone(),
        })
    }

    fn check_variant(
        &self,
        segments: &[String],
        args: &[Expr],
        span: &Span,
        scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
        allow_throw: bool,
    ) -> Result<ExprInfo> {
        let [name, variant] = segments else {
            return Err(Error::diagnostic(
                Diagnostic::new("Ambiguous variant").label(
                    span.clone(),
                    "qualify the variant with its enum name, e.g. `Enum::Variant`",
                ),
            ));
        };
        let Some(info) = self.enums.get(name) else {
            return Err(Error::diagnostic(
                Diagnostic::new("Unknown enum")
                    .label(span.clone(), format!("`{name}` is not a declared enum")),
            ));
        };
        let Some(vinfo) = info.variants.iter().find(|v| v.name == *variant) else {
            return Err(Error::diagnostic(Diagnostic::new("Unknown variant").label(
                span.clone(),
                format!("`{name}` has no variant `{variant}`"),
            )));
        };
        if args.len() != vinfo.fields.len() {
            return Err(Error::diagnostic(
                Diagnostic::new("Wrong number of fields").label(
                    span.clone(),
                    format!(
                        "`{name}.{variant}` takes {} field(s), got {}",
                        vinfo.fields.len(),
                        args.len()
                    ),
                ),
            ));
        }
        let mut effects = EffectRow::default();
        let mut throws = None;
        let mut subst: HashMap<String, Type> = HashMap::new();
        for (arg, field_ty) in args.iter().zip(vinfo.fields.iter()) {
            let actual = self.check_expr(arg, scope, ctx, allow_throw)?;
            // Infer the enum's type arguments from the payload and check the
            // payload against the (possibly generic) field type (spec 0028):
            // `match_type` both binds the type parameters and validates.
            match_type(field_ty, &actual.ty, &mut subst, &actual.span)?;
            effects.union(&actual.effects);
            throws = merge_throws(throws, actual.throws, actual.span)?;
        }
        // Type parameters the payload does not pin (e.g. `R` in `Either`'s
        // `Left(1)`, or every parameter of a payload-less variant like `Nil`)
        // are left `Never`, to be resolved from the expected type via
        // assignability — exactly as `None : Option<Never>` is (spec 0028).
        let type_args = info
            .type_params
            .iter()
            .map(|param| subst.get(param).cloned().unwrap_or(Type::Never))
            .collect();
        Ok(ExprInfo {
            ty: Type::Enum(name.clone(), type_args),
            effects,
            throws,
            span: span.clone(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn check_match(
        &self,
        scrutinee: &Expr,
        arms: &[MatchArm],
        span: &Span,
        scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
        allow_throw: bool,
        expected: Option<&Type>,
    ) -> Result<ExprInfo> {
        let scrut = self.check_expr(scrutinee, scope, ctx, allow_throw)?;
        let variants = self.scrutinee_variants(&scrut.ty, scrutinee.span())?;
        let mut effects = scrut.effects;
        let mut throws = scrut.throws;
        let result = self.check_arms(
            arms,
            &scrut.ty,
            &variants,
            span,
            scope,
            ctx,
            allow_throw,
            &mut effects,
            &mut throws,
            expected,
        )?;
        Ok(ExprInfo {
            ty: result,
            effects,
            throws,
            span: span.clone(),
        })
    }

    fn check_try(
        &self,
        body: &Block,
        arms: &[MatchArm],
        span: &Span,
        scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
    ) -> Result<ExprInfo> {
        // The body runs with throwing calls allowed; thrown errors go to catch.
        let body_info = self.check_block(body, &mut scope.clone(), ctx, true, None)?;
        let caught = body_info.throws.clone();
        let error_ty = caught.clone().unwrap_or(Type::Never);
        let variants = match &caught {
            Some(ty) => self
                .scrutinee_variants(ty, span.clone())
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let mut effects = body_info.effects;
        // The catch arms resolve the error channel; only a re-`throw` re-raises.
        let mut throws = None;
        let mut result = Some(body_info.ty);
        for arm in arms {
            let mut arm_scope = scope.clone();
            self.bind_pattern(&arm.pattern, &error_ty, &variants, &mut arm_scope)?;
            if let Some(guard) = &arm.guard {
                let g = self.check_expr(guard, &mut arm_scope, ctx, false)?;
                expect_assignable(&g.ty, &Type::Bool, g.span.clone())?;
                effects.union(&g.effects);
                throws = merge_throws(throws, g.throws, g.span)?;
            }
            let arm_body = self.check_expr(&arm.body, &mut arm_scope, ctx, false)?;
            result = Some(unify_arm(result, arm_body.ty, arm_body.span.clone())?);
            effects.union(&arm_body.effects);
            throws = merge_throws(throws, arm_body.throws, arm_body.span)?;
        }
        self.check_exhaustive(arms, &variants, &caught, span)?;
        Ok(ExprInfo {
            ty: result.unwrap_or(Type::Unit),
            effects,
            throws,
            span: span.clone(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn check_arms(
        &self,
        arms: &[MatchArm],
        scrut_ty: &Type,
        variants: &[VariantInfo],
        span: &Span,
        scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
        allow_throw: bool,
        effects: &mut EffectRow,
        throws: &mut Option<Type>,
        expected: Option<&Type>,
    ) -> Result<Type> {
        let mut result: Option<Type> = None;
        for arm in arms {
            let mut arm_scope = scope.clone();
            self.bind_pattern(&arm.pattern, scrut_ty, variants, &mut arm_scope)?;
            if let Some(guard) = &arm.guard {
                let g = self.check_expr(guard, &mut arm_scope, ctx, false)?;
                expect_assignable(&g.ty, &Type::Bool, g.span.clone())?;
                effects.union(&g.effects);
                *throws = merge_throws(throws.clone(), g.throws, g.span)?;
            }
            // Each arm produces the match's value, so it inherits the match's
            // expected type (spec 0047): `Nil -> empty()` resolves `Self` here.
            let arm_body =
                self.check_expr_expected(&arm.body, &mut arm_scope, ctx, allow_throw, expected)?;
            result = Some(unify_arm(result, arm_body.ty, arm_body.span.clone())?);
            effects.union(&arm_body.effects);
            *throws = merge_throws(throws.clone(), arm_body.throws, arm_body.span)?;
        }
        self.check_exhaustive(arms, variants, &Some(scrut_ty.clone()), span)?;
        Ok(result.unwrap_or(Type::Unit))
    }

    /// The variants a value of `ty` can take when matched (spec 0005).
    fn scrutinee_variants(&self, ty: &Type, span: Span) -> Result<Vec<VariantInfo>> {
        match ty {
            Type::Enum(name, type_args) => {
                let info = self.enums.get(name).ok_or_else(|| {
                    Error::diagnostic(
                        Diagnostic::new("Unknown enum")
                            .label(span, format!("`{name}` is not a declared enum")),
                    )
                })?;
                // Substitute the scrutinee's concrete type arguments into each
                // variant's field types (spec 0028), so a pattern binding like
                // `Cons(h, t)` on `List<Int>` binds `h: Int`, `t: List<Int>`.
                let subst: HashMap<String, Type> = info
                    .type_params
                    .iter()
                    .cloned()
                    .zip(type_args.iter().cloned())
                    .collect();
                Ok(info
                    .variants
                    .iter()
                    .map(|v| VariantInfo {
                        name: v.name.clone(),
                        fields: v.fields.iter().map(|f| subst_type(f, &subst)).collect(),
                    })
                    .collect())
            }
            _ => Err(Error::diagnostic(Diagnostic::new("Cannot match").label(
                span,
                format!("`match` needs an enum, but found `{ty:?}`"),
            ))),
        }
    }

    fn bind_pattern(
        &self,
        pattern: &Pattern,
        scrut_ty: &Type,
        variants: &[VariantInfo],
        scope: &mut HashMap<String, Type>,
    ) -> Result<()> {
        match pattern {
            Pattern::Wildcard(_) => Ok(()),
            Pattern::Binding { name, span } => {
                self.record(span, scrut_ty, EntryKind::Binding(name.clone()));
                scope.insert(name.clone(), scrut_ty.clone());
                Ok(())
            }
            Pattern::Variant {
                variant,
                fields,
                span,
                ..
            } => {
                let Some(vinfo) = variants.iter().find(|v| &v.name == variant) else {
                    return Err(Error::diagnostic(Diagnostic::new("Unknown variant").label(
                        span.clone(),
                        format!("`{variant}` is not a variant of the matched type"),
                    )));
                };
                if fields.len() != vinfo.fields.len() {
                    return Err(Error::diagnostic(
                        Diagnostic::new("Wrong number of fields").label(
                            span.clone(),
                            format!(
                                "`{variant}` has {} field(s), but the pattern binds {}",
                                vinfo.fields.len(),
                                fields.len()
                            ),
                        ),
                    ));
                }
                for (binding, field_ty) in fields.iter().zip(vinfo.fields.iter()) {
                    if let FieldBinding::Name(name) = binding {
                        scope.insert(name.clone(), field_ty.clone());
                    }
                }
                Ok(())
            }
        }
    }

    /// Enforces that the arms cover every variant (spec 0005). Guarded arms do
    /// not count toward coverage because the guard may fail.
    fn check_exhaustive(
        &self,
        arms: &[MatchArm],
        variants: &[VariantInfo],
        scrutinee: &Option<Type>,
        span: &Span,
    ) -> Result<()> {
        let mut covered = HashSet::new();
        let mut catch_all = false;
        for arm in arms {
            if arm.guard.is_some() {
                continue;
            }
            match &arm.pattern {
                Pattern::Wildcard(_) | Pattern::Binding { .. } => catch_all = true,
                Pattern::Variant { variant, .. } => {
                    covered.insert(variant.clone());
                }
            }
        }
        if catch_all {
            return Ok(());
        }
        // A `try` whose body never throws (`scrutinee == None`) has no variants
        // to cover, so an unreachable catch arm is allowed without a wildcard.
        if scrutinee.is_none() {
            return Ok(());
        }
        let missing: Vec<&str> = variants
            .iter()
            .filter(|v| !covered.contains(&v.name))
            .map(|v| v.name.as_str())
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(Error::diagnostic(
                Diagnostic::new("Non-exhaustive match")
                    .code("non-exhaustive-match")
                    .label(
                        span.clone(),
                        format!("missing case(s): {}", missing.join(", ")),
                    )
                    .help("Add the missing arms, or a wildcard `_ -> ...` arm."),
            ))
        }
    }

    fn check_array(
        &self,
        elements: &[Expr],
        span: &Span,
        scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
        expected_element: Option<&Type>,
        allow_throw: bool,
    ) -> Result<ExprInfo> {
        let mut effects = EffectRow::default();
        let mut throws = None;
        let mut element_ty = expected_element.cloned();
        for element in elements {
            let actual = self.check_expr(element, scope, ctx, allow_throw)?;
            effects.union(&actual.effects);
            throws = merge_throws(throws, actual.throws, actual.span.clone())?;
            match &element_ty {
                Some(expected) => expect_assignable(&actual.ty, expected, actual.span.clone())?,
                None => element_ty = Some(actual.ty),
            }
        }
        let Some(element_ty) = element_ty else {
            return Err(Error::diagnostic(
                Diagnostic::new("Cannot infer array type")
                    .label(span.clone(), "empty array needs an `Array<T>` annotation"),
            ));
        };
        Ok(ExprInfo {
            ty: Type::Array(Box::new(element_ty)),
            effects,
            throws,
            span: span.clone(),
        })
    }

    fn info(&self, ty: Type, span: Span) -> ExprInfo {
        ExprInfo {
            ty,
            effects: EffectRow::default(),
            throws: None,
            span,
        }
    }

    /// Records one span→type fact when index recording is on (spec 0033);
    /// a single branch and no allocation on the normal compile path.
    fn record(&self, span: &Span, ty: &Type, kind: EntryKind) {
        if let Some(index) = &self.type_index {
            index.borrow_mut().push(TypeEntry {
                span: span.clone(),
                ty: ty.clone(),
                kind,
            });
        }
    }

    /// Type-checks a record literal (spec 0006): the name must be a declared
    /// record and the written fields must be exactly its declared fields.
    #[allow(clippy::too_many_arguments)]
    fn check_record_literal(
        &self,
        name: &str,
        name_span: &Span,
        fields: &[(String, Span, Expr)],
        span: &Span,
        scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
        allow_throw: bool,
    ) -> Result<ExprInfo> {
        let Some(record) = self.records.get(name).cloned() else {
            let message = if self.enums.contains_key(name) {
                format!("`{name}` is an enum; construct it as `{name}::Variant(...)`")
            } else {
                format!("`{name}` is not a declared record")
            };
            return Err(Error::diagnostic(
                Diagnostic::new("Unknown record").label(name_span.clone(), message),
            ));
        };
        let declared = &record.fields;
        let mut effects = EffectRow::default();
        let mut throws = None;
        let mut seen = HashSet::new();
        // Infer the record's type arguments (spec 0028) from the field values.
        // `match_type` both binds the type parameters and checks the value
        // against the (possibly generic) field type.
        let mut subst: HashMap<String, Type> = HashMap::new();
        for (field_name, field_span, value) in fields {
            let Some((_, field_ty)) = declared.iter().find(|(n, _)| n == field_name) else {
                return Err(Error::diagnostic(Diagnostic::new("Unknown field").label(
                    field_span.clone(),
                    format!("record `{name}` has no field `{field_name}`"),
                )));
            };
            if !seen.insert(field_name.clone()) {
                return Err(Error::diagnostic(Diagnostic::new("Duplicate field").label(
                    field_span.clone(),
                    format!("field `{field_name}` is written twice"),
                )));
            }
            // An array literal takes its element type from the field so an
            // empty `[]` needs no annotation (mirrors the `let x: Array<T>`
            // path).
            let info = match (value, field_ty) {
                (Expr::Array(elements, array_span), Type::Array(element)) => {
                    self.check_array(elements, array_span, scope, ctx, Some(element), allow_throw)?
                }
                _ => self.check_expr(value, scope, ctx, allow_throw)?,
            };
            effects.union(&info.effects);
            throws = merge_throws(throws, info.throws, info.span.clone())?;
            match_type(field_ty, &info.ty, &mut subst, &info.span)?;
        }
        let missing: Vec<&str> = declared
            .iter()
            .filter(|(n, _)| !seen.contains(n))
            .map(|(n, _)| n.as_str())
            .collect();
        if !missing.is_empty() {
            return Err(Error::diagnostic(Diagnostic::new("Missing fields").label(
                span.clone(),
                format!("record `{name}` needs `{}`", missing.join("`, `")),
            )));
        }
        // Type parameters no field pins are left `Never`, to be resolved from
        // the expected type via assignability — as a payload-less enum variant
        // is (spec 0028).
        let type_args = record
            .type_params
            .iter()
            .map(|param| subst.get(param).cloned().unwrap_or(Type::Never))
            .collect();
        Ok(ExprInfo {
            ty: Type::Enum(name.to_string(), type_args),
            effects,
            throws,
            span: span.clone(),
        })
    }

    /// The type of field `name` on a record value (spec 0006). For a generic
    /// record (spec 0028) the value's type arguments are substituted into the
    /// declared field type, so `first` on a `Pair<Int, String>` is `Int`.
    fn field_type(&self, target_ty: &Type, name: &str, span: &Span) -> Result<Type> {
        if let Type::Enum(type_name, args) = target_ty
            && let Some(record) = self.records.get(type_name)
        {
            if let Some((_, ty)) = record.fields.iter().find(|(n, _)| n == name) {
                let subst: HashMap<String, Type> = record
                    .type_params
                    .iter()
                    .cloned()
                    .zip(args.iter().cloned())
                    .collect();
                return Ok(subst_type(ty, &subst));
            }
            return Err(Error::diagnostic(
                Diagnostic::new("Unknown field")
                    .label(
                        span.clone(),
                        format!("record `{type_name}` has no field `{name}`"),
                    )
                    .help("A method call needs parentheses: `.method(...)`."),
            ));
        }
        Err(Error::diagnostic(Diagnostic::new("Not a record").label(
            span.clone(),
            format!("field access needs a record value, got `{target_ty:?}`"),
        )))
    }

    /// Type-checks a direct call to a generic function (spec 0014). Type
    /// arguments are inferred by matching each declared parameter type (which
    /// may contain `Type::Var`) against the actual argument type, then the
    /// resulting substitution instantiates the return and `throws` types.
    #[allow(clippy::too_many_arguments)]
    fn check_generic_call(
        &self,
        name: &str,
        sig: &FunctionSig,
        args: &[Expr],
        span: &Span,
        scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
        allow_throw: bool,
    ) -> Result<ExprInfo> {
        if args.len() != sig.params.len() {
            return Err(Error::diagnostic(
                Diagnostic::new("Wrong number of arguments").label(
                    span.clone(),
                    format!(
                        "function expects {} argument(s), got {}",
                        sig.params.len(),
                        args.len()
                    ),
                ),
            ));
        }
        let mut effects = sig.effects.clone();
        let mut throws = None;
        let mut subst: HashMap<String, Type> = HashMap::new();
        for (arg, declared) in args.iter().zip(sig.params.iter()) {
            let actual = self.check_expr(arg, scope, ctx, allow_throw)?;
            match_type(declared, &actual.ty, &mut subst, &actual.span)?;
            effects.union(&actual.effects);
            throws = merge_throws(throws, actual.throws, actual.span)?;
        }
        // Every type parameter must be pinned down by the arguments.
        for type_param in &sig.type_params {
            if !subst.contains_key(type_param) {
                return Err(Error::diagnostic(
                    Diagnostic::new("Cannot infer type parameter").label(
                        span.clone(),
                        format!("could not infer type parameter `{type_param}` of `{name}`"),
                    ),
                ));
            }
        }
        // Discharge the callee's bounds (spec 0020) at the inferred type
        // arguments: each `T: Trait` needs an impl (or a matching bound in the
        // caller when `T` is still abstract — bound propagation).
        for bound in &sig.bounds {
            if let Some(concrete) = subst.get(&bound.param).cloned() {
                for tr in &bound.traits {
                    self.check_bound_satisfied(tr, &concrete, ctx, span)?;
                }
            }
        }
        // A throwing call must use `?` or sit inside a `try` block (spec 0011);
        // the error type is the instantiated `throws` clause. In a `@test`
        // body the bare call instead propagates to the harness (spec 0040 T3).
        if let Some(call_error) = &sig.throws {
            if allow_throw {
                let concrete = subst_type(call_error, &subst);
                throws = merge_throws(throws, Some(concrete), span.clone())?;
            } else if !ctx.implicit_try {
                return Err(unhandled_throwing_call(span));
            }
        }
        Ok(ExprInfo {
            ty: subst_type(&sig.ret, &subst),
            effects,
            throws,
            span: span.clone(),
        })
    }
}

/// A generic function may not be used as a first-class value; its type arguments
/// are only fixed at a direct call site (spec 0014).
fn generic_value_error(name: &str, span: &Span) -> Error {
    Error::diagnostic(
        Diagnostic::new("Generic function used as a value")
            .label(
                span.clone(),
                format!("`{name}` is generic and must be called directly"),
            )
            .help("Call it as `name(...)`; generic function values are not supported."),
    )
}

/// A path that matches more than one imported function is ambiguous (spec 0018
/// R5); the diagnostic lists each candidate's full path so the user can qualify
/// the call further.
fn ambiguous_error(path: &str, candidates: &[&FnEntry], span: &Span) -> Error {
    let listed = candidates
        .iter()
        .map(|entry| display_path(&entry.full_path))
        .collect::<Vec<_>>()
        .join(", ");
    Error::diagnostic(
        Diagnostic::new("Ambiguous reference")
            .label(
                span.clone(),
                format!("`{path}` is ambiguous between: {listed}"),
            )
            .help("Qualify the call with its module path, e.g. `module.name(...)`."),
    )
}

/// A bare name that resolves only to an effect operation (spec 0037), which
/// must be called in qualified form; points at the `Effect.op` spelling.
fn effect_op_unqualified_error(name: &str, entry: &FnEntry, span: &Span) -> Error {
    let effect = entry.effect_name.as_deref().unwrap_or_default();
    Error::diagnostic(
        Diagnostic::new("Effect operation called by bare name")
            .label(
                span.clone(),
                format!(
                    "`{name}` is an operation of effect `{effect}`; call it as `{effect}.{name}(...)`"
                ),
            )
            .help(
                "Effect operations are qualified-only (spec 0037); write `Effect.operation(...)` inside a `uses { Effect }` scope.",
            ),
    )
}

/// A bare name that matched only imported public functions (spec 0037 R3):
/// imported functions are called with their module qualifier.
fn bare_imported_error(name: &str, candidates: &[&FnEntry], span: &Span) -> Error {
    let qualified = candidates
        .iter()
        .map(|entry| format!("`{}`", display_path(&entry.full_path)))
        .collect::<Vec<_>>()
        .join(", ");
    Error::diagnostic(
        Diagnostic::new("Imported function called by bare name")
            .label(
                span.clone(),
                format!("`{name}` is imported and must be qualified: {qualified}"),
            )
            .help(
                "Imported functions are called with their module qualifier (spec 0037), e.g. `list.map(...)`.",
            ),
    )
}

/// A qualified path that matched only functions private to another module
/// (spec 0037 R5).
fn private_reference_error(path: &str, candidates: &[&FnEntry], span: &Span) -> Error {
    let label = match candidates
        .first()
        .and_then(|entry| entry.effect_name.as_ref())
    {
        Some(effect) => format!("`{path}` is a private operation of effect `{effect}`"),
        None => format!("`{path}` is private to its module"),
    };
    Error::diagnostic(Diagnostic::new("Private reference").label(span.clone(), label))
}

/// A hint for an effect name that is not in scope (spec 0037): a capitalization
/// near-miss against an in-scope effect, or a known embedded-std effect whose
/// import is missing.
fn effect_scope_hint(name: &str, in_scope: &HashSet<String>) -> Option<String> {
    if let Some(known) = in_scope
        .iter()
        .find(|known| known.eq_ignore_ascii_case(name))
    {
        return Some(format!(
            "Effect names are capitalized (spec 0037); did you mean `{known}`?"
        ));
    }
    match () {
        _ if name == "Io" => {
            Some("Add `import std.io` to bring effect `Io` into scope.".to_string())
        }
        _ if name == "Clock" => {
            Some("Add `import std.clock` to bring effect `Clock` into scope.".to_string())
        }
        _ if name.eq_ignore_ascii_case("io") => Some(
            "The I/O effect is `Io` (spec 0037): add `import std.io` and write `uses { Io }`."
                .to_string(),
        ),
        _ if name.eq_ignore_ascii_case("clock") => Some(
            "The clock effect is `Clock` (spec 0037): add `import std.clock` and write `uses { Clock }`."
                .to_string(),
        ),
        _ => None,
    }
}

/// Combines the error a subexpression may throw with that of its siblings. The
/// throws channel carries a single error type (spec 0011), so two different
/// error types in the same expression are a type error.
fn merge_throws(current: Option<Type>, next: Option<Type>, span: Span) -> Result<Option<Type>> {
    match (current, next) {
        (None, other) | (other, None) => Ok(other),
        (Some(a), Some(b)) if types_compatible(&a, &b) => Ok(Some(a)),
        (Some(b), Some(a)) if types_compatible(&a, &b) => Ok(Some(b)),
        (Some(a), Some(b)) => Err(Error::diagnostic(
            Diagnostic::new("Conflicting error types").label(
                span,
                format!(
                    "this expression mixes errors `{a:?}` and `{b:?}`; use a single error enum"
                ),
            ),
        )),
    }
}

/// The least type both `a` and `b` are assignable to, or `None` if there is
/// none. `Never` (from `throw`/`panic`/`Nil`) joins with anything, taking the
/// other side; two generic types with the same shape join argument-by-argument.
/// This lets `match` arms that each pin a *different* type parameter to `Never`
/// (e.g. `Ok(u) : Result<U, Never>` and `Err(e) : Result<Never, E>`) unify to
/// the fully-applied type (`Result<U, E>`).
fn join_types(a: &Type, b: &Type) -> Option<Type> {
    if a == b {
        return Some(a.clone());
    }
    match (a, b) {
        (Type::Never, other) | (other, Type::Never) => Some(other.clone()),
        (Type::Array(x), Type::Array(y)) => Some(Type::Array(Box::new(join_types(x, y)?))),
        (Type::Enum(an, aargs), Type::Enum(bn, bargs))
            if an == bn && aargs.len() == bargs.len() =>
        {
            let mut out = Vec::with_capacity(aargs.len());
            for (x, y) in aargs.iter().zip(bargs.iter()) {
                out.push(join_types(x, y)?);
            }
            Some(Type::Enum(an.clone(), out))
        }
        _ => None,
    }
}

/// Unifies one `match`/`catch` arm body type with the running result type.
fn unify_arm(current: Option<Type>, ty: Type, span: Span) -> Result<Type> {
    match current {
        None => Ok(ty),
        Some(existing) => join_types(&existing, &ty).ok_or_else(|| {
            Error::diagnostic(Diagnostic::new("Arm type mismatch").label(
                span,
                format!("this arm yields `{ty:?}`, but earlier arms yield `{existing:?}`"),
            ))
        }),
    }
}

/// The trait and method an operator desugars to (spec 0020). Shared with
/// lowering.
pub(crate) fn operator_trait(op: BinaryOp) -> (&'static str, &'static str) {
    match op {
        BinaryOp::Add => ("Add", "add"),
        BinaryOp::Sub => ("Sub", "sub"),
        BinaryOp::Mul => ("Mul", "mul"),
        BinaryOp::Div => ("Div", "div"),
        BinaryOp::Rem => ("Rem", "rem"),
        BinaryOp::Concat => ("Concat", "concat"),
        // Comparisons all resolve through `Eq.eq` / `Ord.lt` (spec 0027): the
        // derived forms impose the same instance requirement as `==` / `<`, and
        // lowering supplies the swap/negation that distinguishes them.
        BinaryOp::Eq | BinaryOp::Ne => ("Eq", "eq"),
        BinaryOp::Lt | BinaryOp::Gt | BinaryOp::Le | BinaryOp::Ge => ("Ord", "lt"),
        // Bitwise operators (spec 0053), each its own operator trait like `+`.
        BinaryOp::BitAnd => ("BitAnd", "bitand"),
        BinaryOp::BitOr => ("BitOr", "bitor"),
        BinaryOp::BitXor => ("BitXor", "bitxor"),
        BinaryOp::Shl => ("Shl", "shl"),
        BinaryOp::Shr => ("Shr", "shr"),
        BinaryOp::UShr => ("UShr", "ushr"),
    }
}

/// A stable key for a type's head constructor, used to look up its unique impl
/// (spec 0020). `None` for types that cannot be an impl target (type variables,
/// `Never`). Shared with lowering.
pub(crate) fn type_head_key(ty: &Type) -> Option<String> {
    let key = match ty {
        Type::Unit => "Unit",
        Type::Bool => "Bool",
        Type::Int => "Int",
        Type::Float => "Float",
        Type::String => "String",
        Type::Char => "Char",
        Type::Bytes => "Bytes",
        Type::Record => "Record",
        Type::Enum(name, _) => return Some(name.clone()),
        Type::Array(_) => "Array",
        Type::Function(_) | Type::OpaqueFunction => "Function",
        Type::Never | Type::Var(_) => return None,
    };
    Some(key.to_string())
}

/// Collapses a list of bounds into a parameter-name -> trait-names map.
fn bounds_map(bounds: &[Bound]) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for bound in bounds {
        map.entry(bound.param.clone())
            .or_default()
            .extend(bound.traits.iter().cloned());
    }
    map
}

fn unsatisfied_bound_error(param: &str, trait_name: &str, span: &Span) -> Error {
    Error::diagnostic(
        Diagnostic::new("Unsatisfied bound")
            .label(
                span.clone(),
                format!("`{param}` does not satisfy `{trait_name}`"),
            )
            .help(format!(
                "Add a `{param}: {trait_name}` bound to the enclosing definition."
            )),
    )
}

fn unsatisfied_bound_error_ty(ty: &Type, trait_name: &str, span: &Span) -> Error {
    Error::diagnostic(
        Diagnostic::new("Unsatisfied bound")
            .label(
                span.clone(),
                format!("`{ty:?}` does not satisfy `{trait_name}`"),
            )
            .help("Implement the trait for this type, or import the module that does."),
    )
}

fn duplicate_function_error(declaration: &Extern) -> Error {
    Error::diagnostic(Diagnostic::new("Duplicate function").label(
        declaration.name_span.clone(),
        format!("`{}` is already defined", declaration.name),
    ))
}

/// A throwing call outside `?`/`try` (spec 0011).
fn unhandled_throwing_call(span: &Span) -> Error {
    Error::diagnostic(
        Diagnostic::new("Unhandled throwing call")
            .label(span.clone(), "this call may throw")
            .help("Use `?` to propagate the error, or wrap it in `try`/`catch`."),
    )
}

/// Validates a `@test` function's signature (spec 0040 T2/T5): no parameters,
/// `Unit` return, no `throws` (the implicit try, T3, makes it meaningless), no
/// type parameters (a test has no call sites to fix them), and no `pub` (a test
/// is excluded from normal builds, so nothing may reference it).
fn check_test_signature(function: &Function, errors: &mut Vec<Error>) {
    let mut push = |message: &str| {
        errors.push(Error::diagnostic(
            Diagnostic::new("Invalid test function")
                .label(function.name_span.clone(), message.to_string()),
        ));
    };
    if function.is_public {
        push("a `@test` fn must not be `pub` (spec 0040)");
    }
    if !function.params.is_empty() {
        push("a `@test` fn takes no parameters (spec 0040)");
    }
    if !matches!(function.ret, Type::Unit) {
        push("a `@test` fn must return `Unit` (spec 0040)");
    }
    if function.throws.is_some() {
        push(
            "a `@test` fn must not declare `throws`: a bare throwing call already propagates as a test failure (spec 0040)",
        );
    }
    if !function.type_params.is_empty() {
        push("a `@test` fn cannot be generic (spec 0040)");
    }
}

/// A method name declared by more than one in-scope trait is ambiguous when
/// called bare (spec 0020, same shape as spec 0018 R5).
fn ambiguous_method_error(method: &str, traits: &[String], span: &Span) -> Error {
    Error::diagnostic(
        Diagnostic::new("Ambiguous method")
            .label(
                span.clone(),
                format!("`{method}` is declared by: {}", traits.join(", ")),
            )
            .help("Qualify the call as `Trait.method(...)`."),
    )
}

/// Fills in defaulted trait methods that an `impl` omits (spec 0020) by cloning
/// the trait's default body into the impl, so later passes treat every impl as
/// fully populated. Runs after import resolution, before type-checking/lowering.
pub(crate) fn expand_trait_defaults(program: &mut Program) {
    let Program {
        traits: trait_decls,
        impls,
        ..
    } = program;
    let by_name: HashMap<&str, &TraitDecl> =
        trait_decls.iter().map(|t| (t.name.as_str(), t)).collect();
    for decl in impls.iter_mut() {
        let Some(trait_decl) = by_name.get(decl.trait_name.as_str()) else {
            continue;
        };
        for tmethod in &trait_decl.methods {
            let Some(default_body) = &tmethod.default_body else {
                continue;
            };
            if decl.methods.iter().any(|m| m.name == tmethod.name) {
                continue;
            }
            decl.methods.push(Function {
                name: tmethod.name.clone(),
                name_span: tmethod.name_span.clone(),
                is_public: false,
                module_path: Vec::new(),
                declared_module: decl.module.clone(),
                effect_name: None,
                type_params: Vec::new(),
                bounds: Vec::new(),
                params: tmethod.params.clone(),
                ret: tmethod.ret.clone(),
                throws: tmethod.throws.clone(),
                effects: tmethod.effects.clone(),
                body: default_body.clone(),
                is_test: false,
            });
        }
    }
}

fn expect_assignable(actual: &Type, expected: &Type, span: Span) -> Result<()> {
    if types_compatible(actual, expected) {
        return Ok(());
    }
    Err(Error::diagnostic(Diagnostic::new("Type mismatch").label(
        span,
        format!("expected `{expected:?}`, but found `{actual:?}`"),
    )))
}

/// Whether a value of type `actual` is acceptable where `expected` is wanted.
/// `Never` (from `throw`/`panic`) is assignable to anything; `Option<Never>`
/// (from a bare `None`) is assignable to any `Option<T>` (spec 0011).
fn types_compatible(actual: &Type, expected: &Type) -> bool {
    if actual == expected {
        return true;
    }
    match (actual, expected) {
        (Type::Never, _) => true,
        (Type::Array(a), Type::Array(e)) => types_compatible(a, e),
        // A generic enum is compatible argument-by-argument (spec 0028), so an
        // unconstrained argument (`Never`, e.g. from `Nil`) unifies with the
        // expected argument the same way `None : Option<Never>` does.
        (Type::Enum(an, aargs), Type::Enum(en, eargs))
            if an == en && aargs.len() == eargs.len() =>
        {
            aargs
                .iter()
                .zip(eargs.iter())
                .all(|(a, e)| types_compatible(a, e))
        }
        // Effect subsumption for function values (spec 0023): a function value is
        // acceptable where a wider one is wanted. Parameters are contravariant,
        // the result is covariant, and the actual effect row must be a subset of
        // the expected one. `throws` is compared exactly for now (spec 0023
        // relaxes only the `uses` row, not the error type).
        (Type::Function(a), Type::Function(e)) if a.params.len() == e.params.len() => {
            a.params
                .iter()
                .zip(e.params.iter())
                .all(|(ap, ep)| types_compatible(ep, ap))
                && types_compatible(&a.ret, &e.ret)
                && a.throws == e.throws
                && a.effects.is_subset_of(&e.effects)
        }
        _ => false,
    }
}

/// Collects the names of the type variables (spec 0014) mentioned anywhere in a
/// type, including nested positions.
fn collect_type_vars(ty: &Type, out: &mut HashSet<String>) {
    match ty {
        Type::Var(name) => {
            out.insert(name.clone());
        }
        Type::Array(inner) => collect_type_vars(inner, out),
        Type::Enum(_, args) => {
            for arg in args {
                collect_type_vars(arg, out);
            }
        }
        Type::Function(function) => {
            for param in &function.params {
                collect_type_vars(param, out);
            }
            collect_type_vars(&function.ret, out);
            if let Some(throws) = &function.throws {
                collect_type_vars(throws, out);
            }
        }
        _ => {}
    }
}

/// Matches a declared type (which may contain type variables) against a concrete
/// `actual` type, recording each type variable's binding in `subst` (spec 0014).
/// Reports a type error on a structural mismatch or an inconsistent binding.
fn match_type(
    declared: &Type,
    actual: &Type,
    subst: &mut HashMap<String, Type>,
    span: &Span,
) -> Result<()> {
    match (declared, actual) {
        (Type::Var(name), _) => {
            match subst.get(name) {
                Some(bound) if bound == actual => {}
                // `Never` (from `throw`/`None`) is too weak to pin a parameter:
                // let a later concrete argument refine the binding, and accept a
                // `Never` argument against an already-concrete binding.
                Some(bound) if *bound == Type::Never => {
                    subst.insert(name.clone(), actual.clone());
                }
                Some(_) if *actual == Type::Never => {}
                Some(bound) => {
                    return Err(Error::diagnostic(Diagnostic::new("Conflicting type argument").label(
                        span.clone(),
                        format!(
                            "type parameter `{name}` is used as both `{bound:?}` and `{actual:?}`"
                        ),
                    )));
                }
                None => {
                    subst.insert(name.clone(), actual.clone());
                }
            }
            Ok(())
        }
        (Type::Array(d), Type::Array(a)) => match_type(d, a, subst, span),
        // Same user-defined generic type on both sides (spec 0028): match the
        // type arguments pairwise, e.g. `List<T>` against `List<Int>` binds `T`.
        (Type::Enum(dn, dargs), Type::Enum(an, aargs))
            if dn == an && dargs.len() == aargs.len() =>
        {
            for (d, a) in dargs.iter().zip(aargs.iter()) {
                match_type(d, a, subst, span)?;
            }
            Ok(())
        }
        (Type::Function(d), Type::Function(a)) if d.params.len() == a.params.len() => {
            for (dp, ap) in d.params.iter().zip(a.params.iter()) {
                match_type(dp, ap, subst, span)?;
            }
            match_type(&d.ret, &a.ret, subst, span)
        }
        // No type variable here: fall back to ordinary assignability.
        _ if types_compatible(actual, declared) => Ok(()),
        _ => Err(Error::diagnostic(Diagnostic::new("Type mismatch").label(
            span.clone(),
            format!("expected `{declared:?}`, but found `{actual:?}`"),
        ))),
    }
}

/// Replaces every type variable in `ty` with its concrete binding from `subst`
/// (spec 0014). A variable with no binding is left as-is. Shared with lowering's
/// monomorphization.
pub(crate) fn subst_type(ty: &Type, subst: &HashMap<String, Type>) -> Type {
    match ty {
        Type::Var(name) => subst.get(name).cloned().unwrap_or_else(|| ty.clone()),
        Type::Array(inner) => Type::Array(Box::new(subst_type(inner, subst))),
        Type::Enum(name, args) => Type::Enum(
            name.clone(),
            args.iter().map(|arg| subst_type(arg, subst)).collect(),
        ),
        Type::Function(function) => Type::Function(FunctionType {
            params: function
                .params
                .iter()
                .map(|param| subst_type(param, subst))
                .collect(),
            ret: Box::new(subst_type(&function.ret, subst)),
            throws: function
                .throws
                .as_ref()
                .map(|throws| Box::new(subst_type(throws, subst))),
            effects: function.effects.clone(),
        }),
        _ => ty.clone(),
    }
}
