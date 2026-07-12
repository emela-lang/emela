use std::collections::{HashMap, HashSet};

use crate::ast::{
    BinaryOp, Block, BlockItem, Bound, EffectRow, Expr, Extern, FieldBinding, Function,
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

/// A declared trait (spec 0020): the set of method signatures a type may satisfy.
#[derive(Debug, Clone)]
struct TraitInfo {
    module: Option<String>,
    methods: Vec<TraitMethodInfo>,
}

#[derive(Debug, Clone)]
struct TraitMethodInfo {
    name: String,
    /// Parameter types, which contain `Type::Var("Self")` in some position.
    params: Vec<Type>,
    ret: Type,
    throws: Option<Type>,
    effects: EffectRow,
    has_default: bool,
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
    ret: &'a Type,
    /// Trait bounds on the enclosing definition's type parameters (spec 0020):
    /// parameter name -> the trait names it is bounded by. Used to allow trait
    /// method calls on a still-abstract type parameter.
    bounds: &'a HashMap<String, Vec<String>>,
    /// The module path of the enclosing function (spec 0018), so a bare-name
    /// call resolves to the referring module's own function before imports.
    /// Empty for a compilation-root function or an impl method.
    module: &'a [String],
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
pub(crate) fn check(program: &Program, require_main: bool) -> (TypedProgram, Vec<Error>) {
    let mut errors = Vec::new();
    let mut checker = Checker {
        table: FnTable::build(program),
        sigs: Vec::new(),
        externs: HashMap::new(),
        enums: HashMap::new(),
        traits: HashMap::new(),
        impls: Vec::new(),
        impls_by: HashMap::new(),
        method_owners: HashMap::new(),
    };
    checker.register_enums(program, &mut errors);
    checker.register_traits(program, &mut errors);
    checker.register_impls(program, &mut errors);
    checker.register_functions(program, &mut errors);
    checker.register_externs(program, &mut errors);
    if require_main && let Err(error) = checker.check_main(program) {
        errors.push(error);
    }
    let mut body_effects = Vec::new();
    for function in &program.functions {
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
    (typed, errors)
}

struct Checker {
    /// Suffix-resolution table over all top-level functions (spec 0018), shared
    /// in structure with lowering.
    table: FnTable,
    /// Each top-level function's signature, indexed in parallel with
    /// `Program::functions` (so `FnEntry::index` indexes it).
    sigs: Vec<FunctionSig>,
    /// Platform functions (`extern fn`, spec 0013), keyed by bare name. They are
    /// always called unqualified and never collide with module imports.
    externs: HashMap<String, FunctionSig>,
    enums: HashMap<String, EnumInfo>,
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
}

impl Checker {
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
        // Payload types may reference other enums (and their own type parameters,
        // which parse to `Type::Var`); validate now that every named type exists
        // and is applied at the right arity (spec 0028).
        for decl in &program.enums {
            for variant in &decl.variants {
                for field in &variant.fields {
                    if let Err(error) = self.validate_type(field, &variant.name_span) {
                        errors.push(error);
                    }
                }
            }
        }
    }

    /// Rejects a type that names an enum that was never declared, or applies a
    /// generic enum at the wrong arity (spec 0005/0028).
    fn validate_type(&self, ty: &Type, span: &Span) -> Result<()> {
        match ty {
            Type::Enum(name, args) => {
                let Some(info) = self.enums.get(name) else {
                    return Err(Error::diagnostic(Diagnostic::new("Unknown type").label(
                        span.clone(),
                        format!("`{name}` is not a declared enum or built-in type"),
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
            Type::Array(inner) | Type::Option(inner) => self.validate_type(inner, span),
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
                // Dispatchability (spec 0020): `Self` must appear in a parameter
                // type so the impl is inferable from arguments; a method with
                // `Self` only in the return type cannot be declared.
                let mut vars = HashSet::new();
                for param in &m.params {
                    collect_type_vars(&param.ty, &mut vars);
                }
                if !vars.contains("Self") {
                    errors.push(Error::diagnostic(
                        Diagnostic::new("Undispatchable trait method")
                            .label(
                                m.name_span.clone(),
                                format!("`{}` must mention `Self` in a parameter type", m.name),
                            )
                            .help("A trait method selects its impl from an argument's type."),
                    ));
                    continue;
                }
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
                return Err(Error::diagnostic(Diagnostic::new("Incomplete impl").label(
                    decl.trait_span.clone(),
                    format!(
                        "missing method `{}` required by `{}`",
                        tmethod.name, decl.trait_name
                    ),
                )));
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
            scope.insert(param.name.clone(), subst_type(&param.ty, &subst));
        }
        let ret = subst_type(&method.ret, &subst);
        let throws = method.throws.as_ref().map(|t| subst_type(t, &subst));
        let bounds = bounds_map(&decl.bounds);
        let ctx = FnCtx {
            throws: &throws,
            ret: &ret,
            bounds: &bounds,
            // Impl methods resolve bare names from the compilation root (their
            // bodies only call unique names — intrinsics and free functions).
            module: &[],
        };
        let body = self.check_block(&method.body, &mut scope, &ctx, false)?;
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
            Type::Enum(name, _) => self.enums.get(name).map(|info| info.module.clone()),
            Type::Int
            | Type::Float
            | Type::String
            | Type::Char
            | Type::Bool
            | Type::Unit
            | Type::Array(_)
            | Type::Option(_) => Some(Some(crate::prelude::CORE_MODULE.to_string())),
            _ => None,
        }
    }

    /// Resolves a trait method call from the argument types (spec 0020): infers
    /// `Self`, discharges the bound (bounded type parameter or concrete impl),
    /// and returns the result type/effects/throws.
    fn dispatch_method(
        &self,
        candidates: &[String],
        method_name: &str,
        args: &[ExprInfo],
        span: &Span,
        ctx: &FnCtx,
        allow_throw: bool,
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
        let Some(self_ty) = subst.get("Self").cloned() else {
            return Err(Error::diagnostic(
                Diagnostic::new("Cannot infer Self").label(
                    span.clone(),
                    format!("could not determine the `Self` type of `{trait_name}.{method_name}`"),
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
            if !allow_throw {
                return Err(Error::diagnostic(
                    Diagnostic::new("Unhandled throwing call")
                        .label(span.clone(), "this call may throw")
                        .help("Use `?` to propagate the error, or wrap it in `try`/`catch`."),
                ));
            }
            throws = merge_throws(throws, Some(subst_type(err, &subst)), span.clone())?;
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
                effects: function.effects.clone(),
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
            if params != entry.params || declaration.ret != entry.ret {
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
            // Intrinsics are identified by their bare name (spec 0021), so the
            // embedded Core Prelude and an imported stdlib module may both
            // declare the same one. Having validated it, a second identical
            // declaration is a harmless no-op rather than a duplicate.
            if self.externs.contains_key(&declaration.name) {
                return Ok(());
            }
            if clashes_function {
                return Err(duplicate_function_error(declaration));
            }
            self.externs.insert(
                declaration.name.clone(),
                FunctionSig {
                    type_params: Vec::new(),
                    bounds: Vec::new(),
                    params,
                    ret: declaration.ret.clone(),
                    throws: None,
                    effects: declaration.effects.clone(),
                },
            );
            return Ok(());
        }
        // A platform function must not collide with anything already defined.
        if self.externs.contains_key(&declaration.name) || clashes_function {
            return Err(duplicate_function_error(declaration));
        }
        let canonical = declaration.canonical();
        let Some(entry) = emela_codegen::platform_lookup(&canonical) else {
            return Err(Error::diagnostic(
                Diagnostic::new("Unknown platform function")
                    .label(
                        declaration.name_span.clone(),
                        format!("`{canonical}` is not a platform function"),
                    )
                    .help("Platform functions are defined by spec 0013."),
            ));
        };
        if params != entry.params || declaration.ret != entry.ret {
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
            FunctionSig {
                // Platform functions are never generic (spec 0013).
                type_params: Vec::new(),
                bounds: Vec::new(),
                params,
                ret: declaration.ret.clone(),
                throws: declaration.throws.clone(),
                effects: declaration.effects.clone(),
            },
        );
        Ok(())
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
        let mut scope = HashMap::new();
        for param in &function.params {
            scope.insert(param.name.clone(), param.ty.clone());
        }
        let bounds = bounds_map(&function.bounds);
        let ctx = FnCtx {
            throws: &function.throws,
            ret: &function.ret,
            bounds: &bounds,
            module: &function.module_path,
        };
        let body = self.check_block(&function.body, &mut scope, &ctx, false)?;
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
    ) -> Result<ExprInfo> {
        let mut scope = outer_scope.clone();
        let mut effects = EffectRow::default();
        let mut throws: Option<Type> = None;
        let mut last = ExprInfo {
            ty: Type::Unit,
            effects: EffectRow::default(),
            throws: None,
            span: block.span.clone(),
        };
        for item in &block.items {
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
                        (Expr::Array(elements, span), Some(Type::Array(element))) => self
                            .check_array(
                                elements,
                                span,
                                &mut scope,
                                ctx,
                                Some(element),
                                allow_throw,
                            )?,
                        _ => self.check_expr(value, &mut scope, ctx, allow_throw)?,
                    };
                    let binding_ty = if let Some(annotation) = ty {
                        expect_assignable(&info.ty, annotation, info.span.clone())?;
                        annotation.clone()
                    } else {
                        info.ty
                    };
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
                    last = self.check_expr(expr, &mut scope, ctx, allow_throw)?;
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
        match expr {
            Expr::Int(_, span) => Ok(self.info(Type::Int, span.clone())),
            Expr::Float(_, span) => Ok(self.info(Type::Float, span.clone())),
            Expr::Bool(_, span) => Ok(self.info(Type::Bool, span.clone())),
            Expr::String(_, span) => Ok(self.info(Type::String, span.clone())),
            Expr::Char(_, span) => Ok(self.info(Type::Char, span.clone())),
            Expr::Array(elements, span) => {
                self.check_array(elements, span, scope, ctx, None, allow_throw)
            }
            Expr::Unit(span) => Ok(self.info(Type::Unit, span.clone())),
            Expr::Var(name, span) => {
                if let Some(ty) = scope.get(name) {
                    Ok(self.info(ty.clone(), span.clone()))
                } else if name == "None" {
                    Ok(self.info(Type::Option(Box::new(Type::Never)), span.clone()))
                } else if let Some(sig) = self.externs.get(name) {
                    Ok(self.info(sig.ty(), span.clone()))
                } else {
                    match self
                        .table
                        .resolve_in(std::slice::from_ref(name), ctx.module)
                    {
                        Resolved::One(entry) => {
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
                self.check_call(callee, args, span, scope, ctx, allow_throw)
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
                    fn_scope.insert(param.name.clone(), param.ty.clone());
                }
                let inner_ctx = FnCtx {
                    throws,
                    ret,
                    bounds: ctx.bounds,
                    module: ctx.module,
                };
                let body_info = self.check_block(body, &mut fn_scope, &inner_ctx, false)?;
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
                self.dispatch_method(&candidates, method, &[left, right], span, ctx, allow_throw)
            }
            Expr::Block(block) => self.check_block(block, scope, ctx, allow_throw),
            Expr::Throw { value, span } => {
                let val = self.check_expr(value, scope, ctx, allow_throw)?;
                Ok(ExprInfo {
                    ty: Type::Never,
                    effects: val.effects,
                    throws: Some(val.ty),
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
                } else if let Type::Option(inner_ty) = &inner.ty {
                    // Option propagation: `?` forwards `None` to the function's
                    // `Option<_>` return (spec 0011).
                    if !matches!(ctx.ret, Type::Option(_)) {
                        return Err(Error::diagnostic(
                            Diagnostic::new("Cannot propagate None").label(
                                span.clone(),
                                "`?` on an Option requires the function to return `Option<_>`",
                            ),
                        ));
                    }
                    Ok(ExprInfo {
                        ty: (**inner_ty).clone(),
                        effects: inner.effects,
                        throws: None,
                        span: span.clone(),
                    })
                } else {
                    Err(Error::diagnostic(Diagnostic::new("Invalid `?`").label(
                        span.clone(),
                        "`?` applies to a throwing call or an `Option` value",
                    )))
                }
            }
            Expr::TypePath { segments, span } => {
                // A `::` type path used as a value (no `(...)`): a no-payload
                // enum variant (specs 0005/0018 R7). Built-in conversions
                // (`Char::from_code`) always take an argument, so a bare one is
                // handled as a call, not a value.
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
                // A dotted path used as a value (no `(...)`): a (qualified)
                // function reference (spec 0018). Enum variants are `::` type
                // paths (`TypePath`), resolved separately.
                match self.table.resolve(segments) {
                    Resolved::One(entry) => {
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
                    // bare name yields this — but handled for totality (spec 0036).
                    Resolved::EffectOpUnqualified(entry) => Err(effect_op_unqualified_error(
                        &segments.join("."),
                        entry,
                        span,
                    )),
                    Resolved::None => {
                        // A dotted path whose head is a declared enum is almost
                        // certainly a variant written with the old `.` spelling;
                        // point the user at the `::` type path (spec 0018 R7).
                        if segments.len() == 2 && self.enums.contains_key(&segments[0]) {
                            return Err(Error::diagnostic(Diagnostic::new("Unknown name").label(
                                span.clone(),
                                format!(
                                    "enum variants use `::`: write `{0}::{1}`, not `{0}.{1}`",
                                    segments[0], segments[1]
                                ),
                            )));
                        }
                        Err(Error::diagnostic(Diagnostic::new("Unknown name").label(
                            span.clone(),
                            format!("`{}` is not defined", segments.join(".")),
                        )))
                    }
                }
            }
            Expr::Match {
                scrutinee,
                arms,
                span,
            } => self.check_match(scrutinee, arms, span, scope, ctx, allow_throw),
            Expr::Try { body, arms, span } => self.check_try(body, arms, span, scope, ctx),
            Expr::If {
                cond,
                then,
                els,
                span,
            } => {
                let cond_info = self.check_expr(cond, scope, ctx, allow_throw)?;
                expect_assignable(&cond_info.ty, &Type::Bool, cond_info.span.clone())?;
                let then_info = self.check_block(then, scope, ctx, allow_throw)?;
                let els_info = self.check_block(els, scope, ctx, allow_throw)?;
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

    fn check_call(
        &self,
        callee: &Expr,
        args: &[Expr],
        span: &Span,
        scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
        allow_throw: bool,
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
            return self.check_call(&method, &method_args, span, scope, ctx, allow_throw);
        }
        // Built-in Option constructor `Some(x)`.
        if let Expr::Var(name, _) = callee
            && name == "Some"
            && !scope.contains_key(name)
            && !self.externs.contains_key(name)
            && matches!(
                self.table
                    .resolve_in(std::slice::from_ref(name), ctx.module),
                Resolved::None
            )
        {
            if args.len() != 1 {
                return Err(Error::diagnostic(
                    Diagnostic::new("Wrong number of arguments").label(
                        span.clone(),
                        format!("`Some` takes 1 argument, got {}", args.len()),
                    ),
                ));
            }
            let arg = self.check_expr(&args[0], scope, ctx, allow_throw)?;
            return Ok(ExprInfo {
                ty: Type::Option(Box::new(arg.ty)),
                effects: arg.effects,
                throws: arg.throws,
                span: span.clone(),
            });
        }
        // Generic function call (spec 0014): a direct call to a generic function
        // infers its type arguments from the argument types. This is handled
        // before the general path because a generic function has no first-class
        // function type to flow through `check_expr`.
        if let Expr::Var(name, _) = callee
            && !scope.contains_key(name)
            && !self.externs.contains_key(name)
            && let Resolved::One(entry) = self
                .table
                .resolve_in(std::slice::from_ref(name), ctx.module)
            && self.sigs[entry.index].is_generic()
        {
            let sig = self.sigs[entry.index].clone();
            return self.check_generic_call(name, &sig, args, span, scope, ctx, allow_throw);
        }
        // A bare trait method call (spec 0020): a name that names a trait method
        // and is not shadowed by a binding, extern, or ordinary function. It is
        // resolved after `FnTable`, so a same-named function still shadows it
        // (spec 0018 R6). The implementation is chosen from the argument types.
        if let Expr::Var(name, _) = callee
            && !scope.contains_key(name)
            && !self.externs.contains_key(name)
            && matches!(
                self.table
                    .resolve_in(std::slice::from_ref(name), ctx.module),
                Resolved::None
            )
            && let Some(candidates) = self.method_owners.get(name)
        {
            let arg_infos = args
                .iter()
                .map(|arg| self.check_expr(arg, scope, ctx, allow_throw))
                .collect::<Result<Vec<_>>>()?;
            return self.dispatch_method(candidates, name, &arg_infos, span, ctx, allow_throw);
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
            );
        }
        // A `::` type-path call target (specs 0005/0017/0018 R7): a built-in
        // conversion (`Char::from_code(n)`) or an enum variant constructor
        // (`Either::Left(x)`). These are resolved through a type, never through
        // the import table.
        if let Expr::TypePath {
            segments,
            span: path_span,
        } = callee
        {
            if let Some(builtin) =
                self.check_char_builtin(segments, args, span, scope, ctx, allow_throw)?
            {
                return Ok(builtin);
            }
            if let Some(builtin) =
                self.check_array_builtin(segments, args, span, scope, ctx, allow_throw)?
            {
                return Ok(builtin);
            }
            if segments.len() == 2 && self.enums.contains_key(&segments[0]) {
                return self.check_variant(segments, args, span, scope, ctx, allow_throw);
            }
            return Err(Error::diagnostic(
                Diagnostic::new("Unknown type path").label(
                    path_span.clone(),
                    format!(
                        "`{}` is not an enum variant or built-in conversion",
                        segments.join("::")
                    ),
                ),
            ));
        }
        // A qualified `.` call target (spec 0018): a (possibly generic) qualified
        // function. A non-generic qualified function falls through to the general
        // path below, where `check_expr` on the path yields its function type.
        if let Expr::Path { segments, .. } = callee {
            match self.table.resolve(segments) {
                Resolved::One(entry) if self.sigs[entry.index].is_generic() => {
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
        for (arg, expected) in args.iter().zip(sig.params.iter()) {
            let actual = self.check_expr(arg, scope, ctx, allow_throw)?;
            expect_assignable(&actual.ty, expected, actual.span.clone())?;
            effects.union(&actual.effects);
            throws = merge_throws(throws, actual.throws, actual.span)?;
        }
        // A throwing call must use `?` or sit inside a `try` block (spec 0011).
        if let Some(call_error) = &sig.throws {
            if !allow_throw {
                return Err(Error::diagnostic(
                    Diagnostic::new("Unhandled throwing call")
                        .label(span.clone(), "this call may throw")
                        .help("Use `?` to propagate the error, or wrap it in `try`/`catch`."),
                ));
            }
            throws = merge_throws(throws, Some((**call_error).clone()), span.clone())?;
        }
        Ok(ExprInfo {
            ty: (*sig.ret).clone(),
            effects,
            throws,
            span: span.clone(),
        })
    }

    /// Type-checks the built-in pure conversions `Char::from_code(Int) -> Char`
    /// and `String::from_char(Char) -> String` (spec 0017). Returns `None` when
    /// the call is not one of them.
    #[allow(clippy::too_many_arguments)]
    fn check_char_builtin(
        &self,
        segments: &[String],
        args: &[Expr],
        span: &Span,
        scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
        allow_throw: bool,
    ) -> Result<Option<ExprInfo>> {
        let [name, variant] = segments else {
            return Ok(None);
        };
        let (arg_ty, ret_ty) = match (name.as_str(), variant.as_str()) {
            ("Char", "from_code") => (Type::Int, Type::Char),
            ("String", "from_char") => (Type::Char, Type::String),
            _ => return Ok(None),
        };
        if args.len() != 1 {
            return Err(Error::diagnostic(
                Diagnostic::new("Wrong number of arguments").label(
                    span.clone(),
                    format!("`{name}::{variant}` takes 1 argument, got {}", args.len()),
                ),
            ));
        }
        let arg = self.check_expr(&args[0], scope, ctx, allow_throw)?;
        expect_assignable(&arg.ty, &arg_ty, arg.span.clone())?;
        Ok(Some(ExprInfo {
            ty: ret_ty,
            effects: arg.effects,
            throws: arg.throws,
            span: span.clone(),
        }))
    }

    /// Type-checks the built-in array operations `Array::length(a) -> Int`,
    /// `Array::get(a, i) -> T` and `Array::push(a, x) -> Array<T>` (spec 0007
    /// companion). Unlike the `Char`/`String` conversions these are polymorphic:
    /// the element type is read from the array argument. Returns `None` when the
    /// call is not one of them.
    #[allow(clippy::too_many_arguments)]
    fn check_array_builtin(
        &self,
        segments: &[String],
        args: &[Expr],
        span: &Span,
        scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
        allow_throw: bool,
    ) -> Result<Option<ExprInfo>> {
        let [name, op] = segments else {
            return Ok(None);
        };
        if name != "Array" {
            return Ok(None);
        }
        let arity = match op.as_str() {
            "length" => 1,
            "get" | "push" => 2,
            _ => return Ok(None),
        };
        if args.len() != arity {
            return Err(Error::diagnostic(
                Diagnostic::new("Wrong number of arguments").label(
                    span.clone(),
                    format!(
                        "`Array::{op}` takes {arity} argument(s), got {}",
                        args.len()
                    ),
                ),
            ));
        }
        let array = self.check_expr(&args[0], scope, ctx, allow_throw)?;
        let Type::Array(elem) = array.ty.clone() else {
            return Err(Error::diagnostic(
                Diagnostic::new("Expected an array").label(
                    array.span.clone(),
                    format!(
                        "`Array::{op}` expects an `Array<_>`, but found `{:?}`",
                        array.ty
                    ),
                ),
            ));
        };
        let elem = *elem;
        let mut effects = array.effects;
        let mut throws = array.throws;
        let ret = match op.as_str() {
            "length" => Type::Int,
            "get" => {
                let index = self.check_expr(&args[1], scope, ctx, allow_throw)?;
                expect_assignable(&index.ty, &Type::Int, index.span.clone())?;
                effects.union(&index.effects);
                throws = merge_throws(throws, index.throws, index.span)?;
                elem
            }
            // `push` returns a fresh array, so `x` must have the element type.
            "push" => {
                let value = self.check_expr(&args[1], scope, ctx, allow_throw)?;
                expect_assignable(&value.ty, &elem, value.span.clone())?;
                effects.union(&value.effects);
                throws = merge_throws(throws, value.throws, value.span)?;
                Type::Array(Box::new(elem))
            }
            _ => unreachable!("arity table covers all ops"),
        };
        Ok(Some(ExprInfo {
            ty: ret,
            effects,
            throws,
            span: span.clone(),
        }))
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

    fn check_match(
        &self,
        scrutinee: &Expr,
        arms: &[MatchArm],
        span: &Span,
        scope: &mut HashMap<String, Type>,
        ctx: &FnCtx,
        allow_throw: bool,
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
        let body_info = self.check_block(body, &mut scope.clone(), ctx, true)?;
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
            let arm_body = self.check_expr(&arm.body, &mut arm_scope, ctx, allow_throw)?;
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
            Type::Option(inner) => Ok(vec![
                VariantInfo {
                    name: "Some".to_string(),
                    fields: vec![(**inner).clone()],
                },
                VariantInfo {
                    name: "None".to_string(),
                    fields: vec![],
                },
            ]),
            _ => Err(Error::diagnostic(Diagnostic::new("Cannot match").label(
                span,
                format!("`match` needs an enum or `Option`, but found `{ty:?}`"),
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
            Pattern::Binding { name, .. } => {
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
        // the error type is the instantiated `throws` clause.
        if let Some(call_error) = &sig.throws {
            if !allow_throw {
                return Err(Error::diagnostic(
                    Diagnostic::new("Unhandled throwing call")
                        .label(span.clone(), "this call may throw")
                        .help("Use `?` to propagate the error, or wrap it in `try`/`catch`."),
                ));
            }
            let concrete = subst_type(call_error, &subst);
            throws = merge_throws(throws, Some(concrete), span.clone())?;
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

/// A bare name that resolves only to an imported effect operation (spec 0036),
/// which must be called in qualified form; points at the `effect.op` spelling.
fn effect_op_unqualified_error(name: &str, entry: &FnEntry, span: &Span) -> Error {
    let full = &entry.full_path;
    // `full_path` is `module_path + [name]`; the effect name is the segment just
    // before the operation, e.g. `io` in `std.io.print`.
    let qualified = if full.len() >= 2 {
        display_path(&full[full.len() - 2..])
    } else {
        name.to_string()
    };
    let effect = full
        .get(full.len().wrapping_sub(2))
        .map(String::as_str)
        .unwrap_or(name);
    Error::diagnostic(
        Diagnostic::new("Effect operation called by bare name")
            .label(
                span.clone(),
                format!(
                    "`{name}` is an operation of effect `{effect}`; call it as `{qualified}(...)`"
                ),
            )
            .help(
                "Effect operations are qualified-only (spec 0036); write `effect.operation(...)`.",
            ),
    )
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
        (Type::Option(x), Type::Option(y)) => Some(Type::Option(Box::new(join_types(x, y)?))),
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
        Type::Record => "Record",
        Type::Enum(name, _) => return Some(name.clone()),
        Type::Array(_) => "Array",
        Type::Option(_) => "Option",
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
                type_params: Vec::new(),
                bounds: Vec::new(),
                params: tmethod.params.clone(),
                ret: tmethod.ret.clone(),
                throws: tmethod.throws.clone(),
                effects: tmethod.effects.clone(),
                is_effect_op: false,
                body: default_body.clone(),
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
        (Type::Option(a), Type::Option(e)) => types_compatible(a, e),
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
        Type::Array(inner) | Type::Option(inner) => collect_type_vars(inner, out),
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
        (Type::Option(d), Type::Option(a)) => match_type(d, a, subst, span),
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
        Type::Option(inner) => Type::Option(Box::new(subst_type(inner, subst))),
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
