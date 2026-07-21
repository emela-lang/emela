//! Lowering: the typed AST -> the `emela-codegen` IR.
//!
//! The IR is fully typed, so every node records the type that the type checker
//! already computed. Lambdas additionally record their captured variables, in
//! a stable order, for closure-converting backends. Calls to `extern fn`
//! platform functions (spec 0013) become `IrExpr::Platform` nodes. Enums,
//! `match`, and the error-handling forms (spec 0005/0011) lower to the IR's
//! `EnumValue`/`Match`/`Throw`/`Try`/`Question`/`Panic` nodes.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use emela_codegen::{
    BinaryOp, FunctionType, IrArm, IrCapture, IrExpr, IrFunction, IrParam, IrPattern, IrProgram,
    Type,
};

use crate::ast::{
    Block, BlockItem, Expr, FieldBinding, Function, ImplDecl, MatchArm, Pattern, Program,
};
use crate::error::Span;
use crate::resolve::{FnTable, Resolved};
use crate::typecheck::{TypedProgram, operator_trait, subst_type, type_head_key};

type Scope = HashMap<String, Type>;

/// What a monomorphization request specializes: either a top-level generic
/// function (spec 0014) or an impl method (spec 0020).
enum TemplateRef {
    Function(usize),
    ImplMethod { impl_ix: usize, method_ix: usize },
}

/// A pending monomorphization: a template specialized at a concrete set of type
/// arguments, identified by its mangled name.
struct MonoRequest {
    /// The mangled name of the specialization, e.g. `identity__Int` or
    /// `Add__Int__add`.
    mangled: String,
    /// The template to specialize.
    template: TemplateRef,
    /// The concrete binding for each type parameter (and `Self` for impls).
    subst: HashMap<String, Type>,
}

#[derive(Default)]
struct MonoState {
    queue: Vec<MonoRequest>,
    /// Mangled names already requested, so each specialization is emitted once.
    requested: HashSet<String>,
}

/// A no-body function in scope: its canonical name, return type, and whether it
/// is an `intrinsic fn` (spec 0021) as opposed to an `extern fn` platform
/// function (spec 0013). Intrinsic calls lower to `IrExpr::Intrinsic`, platform
/// calls to `IrExpr::Platform`. `module`/`effect_name` are the visibility
/// domain (spec 0037), mirroring the type checker's `ExternSig` so both passes
/// resolve a bare name identically.
struct ExternInfo {
    canonical: String,
    /// The declared parameter types. For a generic intrinsic (spec 0021) these
    /// contain `Type::Var`, and are matched against the lowered argument types
    /// to monomorphize `ret` before building the `Intrinsic` node.
    params: Vec<Type>,
    ret: Type,
    /// The error type a fallible platform function throws (spec 0043).
    throws: Option<Type>,
    is_intrinsic: bool,
    module: Option<String>,
    effect_name: Option<String>,
}

/// One variant of a declared enum, with its tag (declaration order) and fields.
struct VariantDef {
    name: String,
    tag: u32,
    fields: Vec<Type>,
}

/// The `Option` lang item on the lowering side (spec 0042): the enum name and
/// its present/absent variant names, so bare `Some`/`None` lower exactly like
/// the qualified `Option::Some` / `Option::None`.
struct OptionLangItem {
    enum_name: String,
    some: String,
    none: String,
}

struct Lowerer<'a> {
    /// Suffix-resolution table over all top-level functions (spec 0018), built
    /// identically to the type checker's so the two passes resolve a path to the
    /// same function — and to the same backend emit name.
    table: FnTable,
    /// All top-level functions, indexed by `FnEntry::index`. Used to build a
    /// resolved call's signature and to fetch generic templates to specialize.
    functions: &'a [Function],
    /// All `impl` blocks, indexed by `TemplateRef::ImplMethod::impl_ix`. Impl
    /// methods are lowered only on demand, when a trait call requests them.
    impls: &'a [ImplDecl],
    externs: HashMap<String, ExternInfo>,
    enums: HashMap<String, Vec<VariantDef>>,
    /// Declared records (spec 0006): name -> fields in declaration order.
    records: HashMap<String, Vec<(String, Type)>>,
    /// The `Option` lang item (spec 0042), if the Core Prelude bound one.
    option_lang_item: Option<OptionLangItem>,
    /// Type parameters of each generic enum (spec 0028), used to substitute the
    /// concrete type arguments into variant field types at construction and
    /// `match`. Empty vec for a non-generic enum.
    enum_type_params: HashMap<String, Vec<String>>,
    /// Method name -> the traits declaring it (spec 0020), for bare dispatch.
    method_owners: HashMap<String, Vec<String>>,
    /// (trait, method) -> the trait method's parameter types and return type.
    /// The parameters contain `Var("Self")` for an argument-dispatched method;
    /// for a return-dispatched method (spec 0047, `empty`) `Self` is in the
    /// return type and is resolved from the call's expected type instead.
    trait_methods: HashMap<(String, String), (Vec<Type>, Type)>,
    /// (trait, type head) -> the unique impl's index in `impls` (spec 0020).
    impls_by: HashMap<(String, String), usize>,
    /// Monomorphization worklist, filled while lowering call sites.
    mono: RefCell<MonoState>,
    /// The type-parameter substitution for the specialization currently being
    /// lowered. Empty while lowering an ordinary (non-generic) function, where
    /// `apply` is the identity.
    subst: RefCell<HashMap<String, Type>>,
    /// The module path of the function currently being lowered, so a bare-name
    /// call resolves within the referring module (spec 0037) — matching the
    /// type checker's `FnCtx::module`.
    current_module: RefCell<Vec<String>>,
    /// The declaring `module` header of the function currently being lowered
    /// (spec 0037): the extern-visibility domain, matching
    /// `FnCtx::declared_module`.
    current_declared_module: RefCell<Option<String>>,
    /// The owning effect of the operation currently being lowered (spec 0037),
    /// matching `FnCtx::effect_name`.
    current_effect: RefCell<Option<String>>,
    /// Counter for fresh temporary names used when desugaring the swapped
    /// comparisons `>` / `<=` (spec 0027), so each site's temporaries are unique.
    cmp_counter: RefCell<usize>,
    /// Counter for fresh temporary names used to sequence non-tail statement
    /// expressions inside blocks.
    stmt_counter: RefCell<usize>,
    /// `true` while lowering a `@test` function's body at an implicit-try
    /// position (spec 0040 T3): a bare throwing call or `throw` here is wrapped
    /// so its error is reported (`io.write_stderr`) and the test trapped,
    /// instead of leaking an unrouted error into the backend. Reset inside
    /// nested function literals and `try` bodies, mirroring the type checker.
    in_test_body: RefCell<bool>,
}

pub(crate) fn lower(program: &Program, typed: &TypedProgram) -> IrProgram {
    let externs: HashMap<String, ExternInfo> = program
        .externs
        .iter()
        .map(|declaration| {
            (
                declaration.name.clone(),
                ExternInfo {
                    canonical: declaration.canonical(),
                    params: declaration.params.iter().map(|p| p.ty.clone()).collect(),
                    ret: declaration.ret.clone(),
                    throws: declaration.throws.clone(),
                    is_intrinsic: declaration.is_intrinsic,
                    module: declaration.module.clone(),
                    effect_name: declaration.effect_name.clone(),
                },
            )
        })
        .collect();
    let enums = program
        .enums
        .iter()
        .map(|decl| {
            let variants = decl
                .variants
                .iter()
                .enumerate()
                .map(|(tag, variant)| VariantDef {
                    name: variant.name.clone(),
                    tag: tag as u32,
                    fields: variant.fields.clone(),
                })
                .collect();
            (decl.name.clone(), variants)
        })
        .collect();
    let enum_type_params: HashMap<String, Vec<String>> = program
        .enums
        .iter()
        .map(|decl| (decl.name.clone(), decl.type_params.clone()))
        .collect();
    let records: HashMap<String, Vec<(String, Type)>> = program
        .records
        .iter()
        .map(|decl| {
            (
                decl.name.clone(),
                decl.fields
                    .iter()
                    .map(|field| (field.name.clone(), field.ty.clone()))
                    .collect(),
            )
        })
        .collect();
    // The `Option` lang item (spec 0042): the enum the Core Prelude tagged with
    // `@lang("option")`. Its shape is validated in type checking, so identify
    // the present (one field) and absent (no fields) variants by arity here.
    let option_lang_item = program
        .enums
        .iter()
        .find(|decl| decl.lang_item.as_deref() == Some("option"))
        .map(|decl| OptionLangItem {
            enum_name: decl.name.clone(),
            some: decl
                .variants
                .iter()
                .find(|v| v.fields.len() == 1)
                .map(|v| v.name.clone())
                .unwrap_or_default(),
            none: decl
                .variants
                .iter()
                .find(|v| v.fields.is_empty())
                .map(|v| v.name.clone())
                .unwrap_or_default(),
        });
    // Trait/impl indexes (spec 0020), mirroring the type checker so a call
    // resolves to the same impl. The type checker already validated everything,
    // so lowering just picks the impl from the (now concrete) argument types.
    let mut method_owners: HashMap<String, Vec<String>> = HashMap::new();
    let mut trait_methods: HashMap<(String, String), (Vec<Type>, Type)> = HashMap::new();
    for decl in &program.traits {
        for method in &decl.methods {
            method_owners
                .entry(method.name.clone())
                .or_default()
                .push(decl.name.clone());
            trait_methods.insert(
                (decl.name.clone(), method.name.clone()),
                (
                    method.params.iter().map(|param| param.ty.clone()).collect(),
                    method.ret.clone(),
                ),
            );
        }
    }
    let mut impls_by: HashMap<(String, String), usize> = HashMap::new();
    for (index, decl) in program.impls.iter().enumerate() {
        if let Some(key) = type_head_key(&decl.target) {
            impls_by.insert((decl.trait_name.clone(), key), index);
        }
    }
    let lowerer = Lowerer {
        table: FnTable::build(program),
        functions: &program.functions,
        impls: &program.impls,
        externs,
        enums,
        records,
        option_lang_item,
        enum_type_params,
        method_owners,
        trait_methods,
        impls_by,
        mono: RefCell::new(MonoState::default()),
        subst: RefCell::new(HashMap::new()),
        current_module: RefCell::new(Vec::new()),
        current_declared_module: RefCell::new(None),
        current_effect: RefCell::new(None),
        cmp_counter: RefCell::new(0),
        stmt_counter: RefCell::new(0),
        in_test_body: RefCell::new(false),
    };

    // Lower the ordinary functions (no substitution); calls to generics enqueue
    // specializations into the worklist. The type checker's signatures equal the
    // AST's, so the ret/throws/effects come straight from it.
    let mut functions: Vec<IrFunction> = program
        .functions
        .iter()
        .zip(typed.functions.iter())
        .enumerate()
        .filter(|(_, (function, _))| function.type_params.is_empty())
        .map(|(index, (function, typed))| {
            let mut scope: Scope = function
                .params
                .iter()
                .zip(typed.params.iter())
                .map(|(param, ty)| (param.name.clone(), ty.clone()))
                .collect();
            *lowerer.current_module.borrow_mut() = function.module_path.clone();
            *lowerer.current_declared_module.borrow_mut() = function.declared_module.clone();
            *lowerer.current_effect.borrow_mut() = function.effect_name.clone();
            *lowerer.in_test_body.borrow_mut() = function.is_test;
            IrFunction {
                // Unique bare names are kept; colliding imports use a mangled
                // full path so same-named functions coexist (spec 0018).
                name: lowerer.table.emit_name(index).to_string(),
                params: function
                    .params
                    .iter()
                    .zip(typed.params.iter())
                    .map(|(param, ty)| IrParam {
                        name: param.name.clone(),
                        ty: ty.clone(),
                    })
                    .collect(),
                ret: typed.ret.clone(),
                throws: typed.throws.clone(),
                effects: typed.effects.clone(),
                body: lowerer
                    .lower_block(&function.body.items, &mut scope, Some(&typed.ret))
                    .0,
            }
        })
        .collect();

    // Drain the monomorphization worklist. Each specialization may itself call
    // other generics or trait methods, enqueueing more, so loop until empty.
    while let Some(request) = lowerer.next_request() {
        let template = match request.template {
            TemplateRef::Function(index) => &lowerer.functions[index],
            TemplateRef::ImplMethod { impl_ix, method_ix } => {
                &lowerer.impls[impl_ix].methods[method_ix]
            }
        };
        *lowerer.subst.borrow_mut() = request.subst;
        *lowerer.current_module.borrow_mut() = template.module_path.clone();
        *lowerer.current_declared_module.borrow_mut() = template.declared_module.clone();
        *lowerer.current_effect.borrow_mut() = template.effect_name.clone();
        // Specializations are never tests (a `@test` fn cannot be generic).
        *lowerer.in_test_body.borrow_mut() = false;
        let specialized = lowerer.lower_named_function(template, request.mangled);
        lowerer.subst.borrow_mut().clear();
        functions.push(specialized);
    }

    // The entry set for import pruning: every function the compilation root
    // itself declares (empty module path) — `main`, `@test`s, and the user's
    // own helpers, which are always emitted. Emit names are bare for the root.
    let root_names: std::collections::HashSet<String> = program
        .functions
        .iter()
        .enumerate()
        .filter(|(_, function)| function.module_path.is_empty())
        .map(|(index, _)| lowerer.table.emit_name(index).to_string())
        .collect();

    let mut program = IrProgram { functions };
    // Drop imported functions unreachable from the root so a library's unused
    // wrappers do not pull their platform imports into the artifact — e.g. a
    // serve-only program must not import the client `http.request` merely
    // because `import std.http` also brings in `Http.get` (spec 0046 S1). The
    // root's own functions are always kept, matching the "all top-level
    // functions are emitted" behavior the rest of the toolchain relies on.
    prune_unreachable_imports(&mut program, &root_names);
    // Direct self-recursive calls in tail position become jumps (spec 0045)
    // before the IR reaches any backend.
    emela_codegen::rewrite_self_tail_calls(&mut program);
    program
}

/// Removes imported IR functions unreachable from the root functions, following
/// the `FunctionRef` names each reachable function mentions (as a call target
/// or a first-class value). Root functions (`root_names`) are always retained;
/// trait dispatch and generics are already resolved to concrete `FunctionRef`s,
/// so an imported function reached through them is kept.
fn prune_unreachable_imports(
    program: &mut IrProgram,
    root_names: &std::collections::HashSet<String>,
) {
    use std::collections::{HashMap, HashSet};
    let by_name: HashMap<&str, &IrFunction> = program
        .functions
        .iter()
        .map(|function| (function.name.as_str(), function))
        .collect();
    let mut reachable: HashSet<String> = HashSet::new();
    let mut queue: Vec<String> = root_names
        .iter()
        .filter(|name| by_name.contains_key(name.as_str()))
        .cloned()
        .collect();
    while let Some(name) = queue.pop() {
        if !reachable.insert(name.clone()) {
            continue;
        }
        if let Some(function) = by_name.get(name.as_str()) {
            emela_codegen::walk(&function.body, &mut |expr| {
                if let IrExpr::FunctionRef { name, .. } = expr
                    && !reachable.contains(name)
                {
                    queue.push(name.clone());
                }
            });
        }
    }
    // Keep every root function unconditionally, plus imported functions the
    // reachability walk marked.
    program.functions.retain(|function| {
        root_names.contains(&function.name) || reachable.contains(&function.name)
    });
}

impl<'a> Lowerer<'a> {
    /// Lowers a function body to an `IrFunction` under the current substitution
    /// (`apply`). For an ordinary function the substitution is empty; for a
    /// specialization it maps the type parameters to concrete types.
    fn lower_named_function(&self, function: &Function, name: String) -> IrFunction {
        let mut scope: Scope = function
            .params
            .iter()
            .map(|param| (param.name.clone(), self.apply(&param.ty)))
            .collect();
        IrFunction {
            name,
            params: function
                .params
                .iter()
                .map(|param| IrParam {
                    name: param.name.clone(),
                    ty: self.apply(&param.ty),
                })
                .collect(),
            ret: self.apply(&function.ret),
            throws: function.throws.as_ref().map(|throws| self.apply(throws)),
            effects: function.effects.clone(),
            body: self
                .lower_block(
                    &function.body.items,
                    &mut scope,
                    Some(&self.apply(&function.ret)),
                )
                .0,
        }
    }

    /// Applies the current specialization substitution to a type. Identity when
    /// no specialization is in progress (the common, non-generic case).
    fn apply(&self, ty: &Type) -> Type {
        let subst = self.subst.borrow();
        if subst.is_empty() {
            ty.clone()
        } else {
            subst_type(ty, &subst)
        }
    }

    /// Records a generic-function specialization to emit, deduped by mangled name.
    fn request_specialization(
        &self,
        mangled: &str,
        template_index: usize,
        subst: HashMap<String, Type>,
    ) {
        self.enqueue(mangled, TemplateRef::Function(template_index), subst);
    }

    fn enqueue(&self, mangled: &str, template: TemplateRef, subst: HashMap<String, Type>) {
        let mut mono = self.mono.borrow_mut();
        if mono.requested.insert(mangled.to_string()) {
            mono.queue.push(MonoRequest {
                mangled: mangled.to_string(),
                template,
                subst,
            });
        }
    }

    /// Lowers a trait method call (spec 0020) to a direct call of the specialized
    /// impl method. `candidates` are the traits that might own the method (one
    /// for a qualified call, possibly several for a bare name — the one with a
    /// matching impl wins; the type checker has guaranteed exactly one applies).
    fn lower_trait_call(
        &self,
        candidates: &[String],
        method_name: &str,
        args: &[Expr],
        scope: &mut Scope,
        expected: Option<&Type>,
    ) -> (IrExpr, Type) {
        let lowered: Vec<(IrExpr, Type)> =
            args.iter().map(|arg| self.lower_expr(arg, scope)).collect();
        self.resolve_impl_call(candidates, method_name, lowered, expected)
    }

    /// Resolves already-lowered operands to a concrete impl method and emits the
    /// call. Shared by trait method calls and desugared operators. `expected` is
    /// the call's (already concrete) expected type, used to resolve a
    /// return-dispatched method like `empty()` (spec 0047).
    fn resolve_impl_call(
        &self,
        candidates: &[String],
        method_name: &str,
        lowered: Vec<(IrExpr, Type)>,
        expected: Option<&Type>,
    ) -> (IrExpr, Type) {
        // Argument types are concrete here: a specialization substitutes every
        // type variable before its body is lowered (spec 0014/0020 erasure).
        let arg_tys: Vec<Type> = lowered.iter().map(|(_, ty)| self.apply(ty)).collect();
        for trait_name in candidates {
            let Some((params, ret)) = self
                .trait_methods
                .get(&(trait_name.clone(), method_name.to_string()))
            else {
                continue;
            };
            let mut subst = HashMap::new();
            for (declared, actual) in params.iter().zip(arg_tys.iter()) {
                infer_subst(declared, actual, &mut subst);
            }
            // Return dispatch (spec 0047): resolve `Self` from the expected type.
            if !subst.contains_key("Self")
                && let Some(exp) = expected
            {
                infer_subst(ret, &self.apply(exp), &mut subst);
            }
            let Some(self_ty) = subst.get("Self").cloned() else {
                continue;
            };
            let Some(key) = type_head_key(&self_ty) else {
                continue;
            };
            let Some(&impl_ix) = self.impls_by.get(&(trait_name.clone(), key)) else {
                continue;
            };
            return self.emit_impl_call(trait_name, method_name, &self_ty, impl_ix, lowered);
        }
        // Unreachable after a successful type check.
        (IrExpr::Unit, Type::Unit)
    }

    /// Lowers a binary operator on already-lowered operands. Arithmetic, `==`,
    /// `<`, and `++` dispatch straight to their trait method (spec 0020). The
    /// derived comparisons `!= > <= >=` desugar to `Eq.eq` / `Ord.lt` per spec
    /// 0027 while preserving left-to-right operand evaluation.
    fn lower_binary(
        &self,
        op: BinaryOp,
        left: (IrExpr, Type),
        right: (IrExpr, Type),
    ) -> (IrExpr, Type) {
        match op {
            // `a != b` == `!(a == b)`, `a >= b` == `!(a < b)`: no operand swap, so
            // dispatching `(left, right)` already evaluates `a` before `b`.
            BinaryOp::Ne => {
                let (cmp, _) =
                    self.resolve_impl_call(&["Eq".to_string()], "eq", vec![left, right], None);
                (bool_not(cmp), Type::Bool)
            }
            BinaryOp::Ge => {
                let (cmp, _) =
                    self.resolve_impl_call(&["Ord".to_string()], "lt", vec![left, right], None);
                (bool_not(cmp), Type::Bool)
            }
            // `a > b` == `b < a`, `a <= b` == `!(b < a)`: the swap would reorder
            // evaluation, so bind both operands to temporaries in source order.
            BinaryOp::Gt => self.lower_swapped_lt(left, right, false),
            BinaryOp::Le => self.lower_swapped_lt(left, right, true),
            _ => {
                let (trait_name, method) = operator_trait(op);
                self.resolve_impl_call(&[trait_name.to_string()], method, vec![left, right], None)
            }
        }
    }

    /// Emits `b < a` (negated for `<=`) while keeping `a` evaluated before `b`
    /// (spec 0027): binds the operands to fresh temporaries in source order, then
    /// compares them swapped.
    fn lower_swapped_lt(
        &self,
        left: (IrExpr, Type),
        right: (IrExpr, Type),
        negate: bool,
    ) -> (IrExpr, Type) {
        let (left_ir, left_ty) = left;
        let (right_ir, right_ty) = right;
        let left_ty = self.apply(&left_ty);
        let right_ty = self.apply(&right_ty);
        let name_a = self.fresh_cmp_temp();
        let name_b = self.fresh_cmp_temp();
        let (cmp, _) = self.resolve_impl_call(
            &["Ord".to_string()],
            "lt",
            vec![
                (
                    IrExpr::Var {
                        name: name_b.clone(),
                        ty: right_ty.clone(),
                    },
                    right_ty.clone(),
                ),
                (
                    IrExpr::Var {
                        name: name_a.clone(),
                        ty: left_ty.clone(),
                    },
                    left_ty.clone(),
                ),
            ],
            None,
        );
        let body = if negate { bool_not(cmp) } else { cmp };
        // let name_a = <a> in let name_b = <b> in (b < a): evaluates a then b.
        let inner = IrExpr::Let {
            name: name_b,
            value_ty: right_ty,
            value: Box::new(right_ir),
            next: Box::new(body),
        };
        let outer = IrExpr::Let {
            name: name_a,
            value_ty: left_ty,
            value: Box::new(left_ir),
            next: Box::new(inner),
        };
        (outer, Type::Bool)
    }

    /// A fresh, collision-proof temporary name for comparison desugaring.
    fn fresh_cmp_temp(&self) -> String {
        let mut counter = self.cmp_counter.borrow_mut();
        let name = format!("$cmp{}", *counter);
        *counter += 1;
        name
    }

    /// A fresh, collision-proof temporary name for sequencing non-tail block
    /// expressions while preserving source order.
    fn fresh_stmt_temp(&self) -> String {
        let mut counter = self.stmt_counter.borrow_mut();
        let name = format!("$stmt{}", *counter);
        *counter += 1;
        name
    }

    /// Requests the specialization of an impl method for `self_ty` and emits a
    /// direct call to its mangled name, e.g. `Add__Int__add` (spec 0020).
    fn emit_impl_call(
        &self,
        trait_name: &str,
        method_name: &str,
        self_ty: &Type,
        impl_ix: usize,
        lowered: Vec<(IrExpr, Type)>,
    ) -> (IrExpr, Type) {
        let decl = &self.impls[impl_ix];
        let method_ix = decl
            .methods
            .iter()
            .position(|m| m.name == method_name)
            .expect("impl provides the method (checked)");
        let method = &decl.methods[method_ix];
        // The impl's own type parameters (for a parameterized instance) are
        // inferred from the target against the concrete `Self` type.
        let mut subst = HashMap::new();
        subst.insert("Self".to_string(), self_ty.clone());
        infer_subst(&decl.target, self_ty, &mut subst);
        let mangled = mangle_impl_method(trait_name, self_ty, method_name);
        let sig = FunctionType {
            params: method
                .params
                .iter()
                .map(|param| subst_type(&param.ty, &subst))
                .collect(),
            ret: Box::new(subst_type(&method.ret, &subst)),
            throws: method
                .throws
                .as_ref()
                .map(|throws| Box::new(subst_type(throws, &subst))),
            effects: method.effects.clone(),
        };
        self.enqueue(
            &mangled,
            TemplateRef::ImplMethod { impl_ix, method_ix },
            subst,
        );
        let ret = (*sig.ret).clone();
        (
            IrExpr::Call {
                callee: Box::new(IrExpr::FunctionRef { name: mangled, sig }),
                args: lowered.into_iter().map(|(expr, _)| expr).collect(),
                ret: ret.clone(),
            },
            ret,
        )
    }

    /// The signature of a non-generic top-level function, by its index in
    /// `Program::functions`. Used to build a `FunctionRef` for a resolved call.
    fn fn_type(&self, index: usize) -> FunctionType {
        let function = &self.functions[index];
        FunctionType {
            params: function.params.iter().map(|p| p.ty.clone()).collect(),
            ret: Box::new(function.ret.clone()),
            throws: function.throws.clone().map(Box::new),
            effects: function.effects.clone(),
        }
    }

    fn next_request(&self) -> Option<MonoRequest> {
        self.mono.borrow_mut().queue.pop()
    }

    fn lower_block(
        &self,
        items: &[BlockItem],
        scope: &mut Scope,
        expected: Option<&Type>,
    ) -> (IrExpr, Type) {
        match items.split_first() {
            None => (IrExpr::Unit, Type::Unit),
            // The tail expression is the block's value, so it inherits the
            // block's expected type (spec 0047).
            Some((BlockItem::Expr(expr), [])) => self.lower_expr_expected(expr, scope, expected),
            Some((BlockItem::Expr(expr), rest)) => {
                let (value, value_ty) = self.lower_expr(expr, scope);
                let (next, next_ty) = self.lower_block(rest, scope, expected);
                (
                    IrExpr::Let {
                        name: self.fresh_stmt_temp(),
                        value_ty,
                        value: Box::new(value),
                        next: Box::new(next),
                    },
                    next_ty,
                )
            }
            Some((
                BlockItem::Let {
                    name, ty, value, ..
                },
                rest,
            )) => {
                // The annotation may mention this function's type parameters in
                // a specialization, so resolve it under the substitution.
                let annotated = ty.as_ref().map(|ty| self.apply(ty));
                let expected_elem = match (value, &annotated) {
                    (Expr::Array(_, _), Some(Type::Array(element))) => Some(element.as_ref()),
                    _ => None,
                };
                let (value, inferred) = match value {
                    Expr::Array(elements, _) => self.lower_array(elements, scope, expected_elem),
                    // An annotated `let` gives its value an expected type, so a
                    // return-dispatched `empty()` resolves here (spec 0047).
                    _ => self.lower_expr_expected(value, scope, annotated.as_ref()),
                };
                let value_ty = annotated.unwrap_or(inferred);
                scope.insert(name.clone(), value_ty.clone());
                let (next, next_ty) = self.lower_block(rest, scope, expected);
                (
                    IrExpr::Let {
                        name: name.clone(),
                        value_ty,
                        value: Box::new(value),
                        next: Box::new(next),
                    },
                    next_ty,
                )
            }
        }
    }

    fn lower_array(
        &self,
        elements: &[Expr],
        scope: &mut Scope,
        expected_elem: Option<&Type>,
    ) -> (IrExpr, Type) {
        let lowered = elements
            .iter()
            .map(|element| self.lower_expr(element, scope))
            .collect::<Vec<_>>();
        let elem_ty = lowered
            .first()
            .map(|(_, ty)| ty.clone())
            .or_else(|| expected_elem.cloned())
            .unwrap_or(Type::Unit);
        (
            IrExpr::Array {
                elem_ty: elem_ty.clone(),
                elems: lowered.into_iter().map(|(expr, _)| expr).collect(),
            },
            Type::Array(Box::new(elem_ty)),
        )
    }

    /// Lowers a record-literal field value, passing the declared field type as
    /// the expected element type so an empty `[]` array is typed from the field
    /// (mirroring the checker).
    fn lower_field_value(&self, value: &Expr, field_ty: &Type, scope: &mut Scope) -> IrExpr {
        if let (Expr::Array(elements, _), Type::Array(element)) = (value, field_ty) {
            return self.lower_array(elements, scope, Some(element)).0;
        }
        self.lower_expr(value, scope).0
    }

    /// The declaration-order index and type of `field` on a record-typed value
    /// (spec 0006). The type checker has already validated the access.
    fn record_field(&self, ty: &Type, field: &str) -> (u32, Type) {
        let Type::Enum(name, _) = ty else {
            unreachable!("field access on a non-record was rejected by the checker");
        };
        let fields = &self.records[name];
        let index = fields
            .iter()
            .position(|(n, _)| n == field)
            .expect("field validated by the checker");
        (index as u32, fields[index].1.clone())
    }

    /// Lowers a record literal (spec 0006). Fields are evaluated in written
    /// order (spec 0003's left-to-right); when that differs from declaration
    /// order they go through temporaries so the stored order is declaration
    /// order either way.
    fn lower_record_literal(
        &self,
        name: &str,
        fields: &[(String, Span, Expr)],
        scope: &mut Scope,
    ) -> (IrExpr, Type) {
        let declared = self.records[name].clone();
        let ty = Type::Enum(name.to_string(), Vec::new());
        let written_in_decl_order = fields.len() == declared.len()
            && fields
                .iter()
                .zip(&declared)
                .all(|((written, _, _), (decl, _))| written == decl);
        if written_in_decl_order {
            let lowered = fields
                .iter()
                .zip(&declared)
                .map(|((_, _, value), (_, field_ty))| {
                    self.lower_field_value(value, field_ty, scope)
                })
                .collect();
            return (
                IrExpr::RecordValue {
                    ty: ty.clone(),
                    fields: lowered,
                },
                ty,
            );
        }
        let mut temps: HashMap<String, String> = HashMap::new();
        let mut lets: Vec<(String, Type, IrExpr)> = Vec::new();
        for (field_name, _, value) in fields {
            let (_, field_ty) = declared
                .iter()
                .find(|(n, _)| n == field_name)
                .expect("field validated by the checker");
            let ir = self.lower_field_value(value, field_ty, scope);
            let value_ty = field_ty.clone();
            let temp = {
                let mut counter = self.stmt_counter.borrow_mut();
                let temp = format!("$fld{}", *counter);
                *counter += 1;
                temp
            };
            temps.insert(field_name.clone(), temp.clone());
            lets.push((temp, value_ty, ir));
        }
        let record = IrExpr::RecordValue {
            ty: ty.clone(),
            fields: declared
                .iter()
                .map(|(field_name, field_ty)| IrExpr::Var {
                    name: temps[field_name].clone(),
                    ty: field_ty.clone(),
                })
                .collect(),
        };
        let ir = lets
            .into_iter()
            .rev()
            .fold(record, |next, (temp, value_ty, value)| IrExpr::Let {
                name: temp,
                value_ty,
                value: Box::new(value),
                next: Box::new(next),
            });
        (ir, ty)
    }

    fn lower_expr(&self, expr: &Expr, scope: &mut Scope) -> (IrExpr, Type) {
        self.lower_expr_expected(expr, scope, None)
    }

    /// Like `lower_expr` but with the (already concrete) expected type from the
    /// surrounding context, used to resolve a return-dispatched `empty()` to its
    /// impl (spec 0047). Threaded through the same positions as the checker.
    fn lower_expr_expected(
        &self,
        expr: &Expr,
        scope: &mut Scope,
        expected: Option<&Type>,
    ) -> (IrExpr, Type) {
        match expr {
            Expr::Int(value, _) => (IrExpr::Int(*value), Type::Int),
            Expr::Float(value, _) => (IrExpr::Float(*value), Type::Float),
            Expr::Bool(value, _) => (IrExpr::Bool(*value), Type::Bool),
            Expr::String(value, _) => (IrExpr::String(value.clone()), Type::String),
            Expr::Char(value, _) => (IrExpr::Char(*value as u32), Type::Char),
            Expr::Array(elements, _) => self.lower_array(elements, scope, None),
            Expr::RecordLiteral { name, fields, .. } => {
                self.lower_record_literal(name, fields, scope)
            }
            Expr::Field { target, name, .. } => {
                let (target_ir, target_ty) = self.lower_expr(target, scope);
                let (index, field_ty) = self.record_field(&target_ty, name);
                (
                    IrExpr::FieldAccess {
                        target: Box::new(target_ir),
                        index,
                        field_ty: field_ty.clone(),
                    },
                    field_ty,
                )
            }
            Expr::Unit(_) => (IrExpr::Unit, Type::Unit),
            Expr::Var(name, _) => {
                if let Some(ty) = scope.get(name) {
                    (
                        IrExpr::Var {
                            name: name.clone(),
                            ty: ty.clone(),
                        },
                        ty.clone(),
                    )
                } else if let Some(li) = &self.option_lang_item
                    && *name == li.none
                {
                    // Bare `None` (spec 0042 O3): the absent variant of the
                    // `option` lang item, lowered like `Option::None`.
                    let enum_name = li.enum_name.clone();
                    let def = self.enums[&enum_name]
                        .iter()
                        .find(|v| v.name == *name)
                        .expect("option lang item validated in type checking");
                    let args = self.enum_type_args(&enum_name, def, &[]);
                    let ty = Type::Enum(enum_name.clone(), args);
                    (
                        IrExpr::EnumValue {
                            ty: ty.clone(),
                            variant: name.clone(),
                            tag: def.tag,
                            payload: Vec::new(),
                        },
                        ty,
                    )
                } else if let Resolved::One(entry) = self.table.resolve_in(
                    std::slice::from_ref(name),
                    self.current_module.borrow().as_slice(),
                ) && !entry.is_generic
                {
                    let sig = self.fn_type(entry.index);
                    (
                        IrExpr::FunctionRef {
                            name: entry.emit_name.clone(),
                            sig: sig.clone(),
                        },
                        Type::Function(sig),
                    )
                } else {
                    (
                        IrExpr::Var {
                            name: name.clone(),
                            ty: Type::Unit,
                        },
                        Type::Unit,
                    )
                }
            }
            Expr::Call { callee, args, .. } => {
                let (ir, ty) = self.lower_call(callee, args, scope, expected);
                self.maybe_wrap_test_site(ir, ty)
            }
            Expr::Fn {
                params,
                ret,
                throws,
                effects,
                body,
                ..
            } => {
                let captures = self.lambda_captures(params, body, scope);
                let mut fn_scope = scope.clone();
                for param in params {
                    fn_scope.insert(param.name.clone(), param.ty.clone());
                }
                // A literal's body follows the ordinary throwing rules even
                // inside a `@test` body (spec 0040 T3).
                let saved = self.in_test_body.replace(false);
                let (body, _) = self.lower_block(&body.items, &mut fn_scope, Some(ret));
                self.in_test_body.replace(saved);
                let ir_params: Vec<IrParam> = params
                    .iter()
                    .map(|param| IrParam {
                        name: param.name.clone(),
                        ty: param.ty.clone(),
                    })
                    .collect();
                let signature = FunctionType {
                    params: ir_params.iter().map(|param| param.ty.clone()).collect(),
                    ret: Box::new(ret.clone()),
                    throws: throws.clone().map(Box::new),
                    effects: effects.clone(),
                };
                (
                    IrExpr::Fn {
                        params: ir_params,
                        ret: ret.clone(),
                        throws: throws.clone(),
                        effects: effects.clone(),
                        captures,
                        body: Box::new(body),
                    },
                    Type::Function(signature),
                )
            }
            Expr::Binary {
                op, left, right, ..
            } => {
                let left = self.lower_expr(left, scope);
                let right = self.lower_expr(right, scope);
                let (ir, ty) = self.lower_binary(*op, left, right);
                self.maybe_wrap_test_site(ir, ty)
            }
            Expr::Block(block) => self.lower_block(&block.items, &mut scope.clone(), expected),
            Expr::If {
                cond, then, els, ..
            } => {
                let (cond_ir, _) = self.lower_expr(cond, scope);
                let (then_ir, then_ty) =
                    self.lower_block(&then.items, &mut scope.clone(), expected);
                let (els_ir, els_ty) = self.lower_block(&els.items, &mut scope.clone(), expected);
                let ty = pick_ty([then_ty, els_ty].into_iter());
                (
                    IrExpr::If {
                        cond: Box::new(cond_ir),
                        then: Box::new(then_ir),
                        els: Box::new(els_ir),
                        ty: ty.clone(),
                    },
                    ty,
                )
            }
            Expr::Throw { value, .. } => {
                let (value, _) = self.lower_expr(value, scope);
                let ir = IrExpr::Throw {
                    value: Box::new(value),
                };
                self.maybe_wrap_test_site(ir, Type::Never)
            }
            Expr::Panic { message, .. } => {
                let (message, _) = self.lower_expr(message, scope);
                (
                    IrExpr::Panic {
                        message: Box::new(message),
                    },
                    Type::Never,
                )
            }
            Expr::Question { value, .. } => {
                // `?` applies only to throwing calls (spec 0011/0042); the type
                // checker rejects it on any non-throwing value, so lowering just
                // forwards the (already-unwrapped) success value.
                let (value, value_ty) = self.lower_expr(value, scope);
                (
                    IrExpr::Question {
                        value: Box::new(value),
                        ty: value_ty.clone(),
                    },
                    value_ty,
                )
            }
            Expr::TypePath { segments, .. } => {
                // A `::` type path with no `(...)`: a no-payload enum variant
                // (specs 0005/0018 R7). Built-in conversions always carry args
                // and are handled in `lower_call`.
                if let [enum_name, variant] = segments.as_slice()
                    && let Some(variants) = self.enums.get(enum_name)
                    && let Some(def) = variants.iter().find(|v| v.name == *variant)
                {
                    // A payload-less variant pins no type parameters, so every
                    // argument is `Never` (spec 0028), e.g. `List::Nil : List<Never>`.
                    let args = self.enum_type_args(enum_name, def, &[]);
                    let ty = Type::Enum(enum_name.clone(), args);
                    return (
                        IrExpr::EnumValue {
                            ty: ty.clone(),
                            variant: variant.clone(),
                            tag: def.tag,
                            payload: Vec::new(),
                        },
                        ty,
                    );
                }
                (IrExpr::Unit, Type::Unit)
            }
            Expr::Path { segments, .. } => {
                // A dotted head that is a local value is record field access
                // (spec 0006), mirroring the checker.
                if let Some(head_ty) = scope.get(&segments[0]).cloned() {
                    let mut ir = IrExpr::Var {
                        name: segments[0].clone(),
                        ty: head_ty.clone(),
                    };
                    let mut ty = head_ty;
                    for segment in &segments[1..] {
                        let (index, field_ty) = self.record_field(&ty, segment);
                        ir = IrExpr::FieldAccess {
                            target: Box::new(ir),
                            index,
                            field_ty: field_ty.clone(),
                        };
                        ty = field_ty;
                    }
                    return (ir, ty);
                }
                // A dotted path with no `(...)`: an effect operation or a
                // module-qualified function used as a value (spec 0037). Enum
                // variants are `::` type paths, handled above.
                if let Resolved::One(entry) = self
                    .table
                    .resolve_in(segments, self.current_module.borrow().as_slice())
                    && !entry.is_generic
                {
                    let sig = self.fn_type(entry.index);
                    return (
                        IrExpr::FunctionRef {
                            name: entry.emit_name.clone(),
                            sig: sig.clone(),
                        },
                        Type::Function(sig),
                    );
                }
                (IrExpr::Unit, Type::Unit)
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                let (scrutinee_ir, scrutinee_ty) = self.lower_expr(scrutinee, scope);
                let variants = self.variants_of(&scrutinee_ty);
                let ir_arms: Vec<IrArm> = arms
                    .iter()
                    .map(|arm| self.lower_arm(arm, &scrutinee_ty, &variants, scope, expected))
                    .collect();
                let ty = pick_ty(ir_arms.iter().map(|arm| arm.body.ty()));
                (
                    IrExpr::Match {
                        scrutinee: Box::new(scrutinee_ir),
                        arms: ir_arms,
                        ty: ty.clone(),
                    },
                    ty,
                )
            }
            Expr::Try { body, arms, .. } => {
                // Inside a `try` body a thrown error routes to `catch`, not to
                // the test harness (spec 0040 T3); the arms are back at an
                // implicit-try position (an uncaught re-`throw` fails the test).
                let saved = self.in_test_body.replace(false);
                let (body_ir, body_ty) = self.lower_block(&body.items, &mut scope.clone(), None);
                self.in_test_body.replace(saved);
                let error_ty = body_error_ty(&body_ir).unwrap_or(Type::Never);
                let variants = self.variants_of(&error_ty);
                let ir_arms: Vec<IrArm> = arms
                    .iter()
                    .map(|arm| self.lower_arm(arm, &error_ty, &variants, scope, None))
                    .collect();
                let ty = pick_ty(
                    std::iter::once(body_ty).chain(ir_arms.iter().map(|arm| arm.body.ty())),
                );
                (
                    IrExpr::Try {
                        body: Box::new(body_ir),
                        arms: ir_arms,
                        ty: ty.clone(),
                        err_name: None,
                    },
                    ty,
                )
            }
        }
    }

    /// Wraps a lowered expression when it is a bare throwing site in a `@test`
    /// body (spec 0040 T3): `try { <site> } catch { e -> report(e); trap }`.
    /// The report renders the error with its `Show` impl when one is in scope
    /// (spec 0040 C7) and writes it to stderr; the trap (an IR `Panic`) is what
    /// the runner observes as the test's failure (spec 0040 C4). Identity for
    /// non-throwing sites and outside test bodies.
    fn maybe_wrap_test_site(&self, ir: IrExpr, ty: Type) -> (IrExpr, Type) {
        if !*self.in_test_body.borrow() {
            return (ir, ty);
        }
        let Some(error_ty) = site_error_ty(&ir) else {
            return (ir, ty);
        };
        let error_var = "__test_error".to_string();
        let label = error_type_label(&error_ty);
        // `threw <Type>` — plus `: <Show rendering>` when an impl is in scope.
        let message = match type_head_key(&error_ty).filter(|key| {
            self.impls_by
                .contains_key(&("Show".to_string(), key.clone()))
        }) {
            Some(_) => {
                let (rendered, _) = self.resolve_impl_call(
                    &["Show".to_string()],
                    "to_string",
                    vec![(
                        IrExpr::Var {
                            name: error_var.clone(),
                            ty: error_ty.clone(),
                        },
                        error_ty.clone(),
                    )],
                    None,
                );
                IrExpr::Concat {
                    left: Box::new(IrExpr::String(format!("threw {label}: "))),
                    right: Box::new(rendered),
                }
            }
            None => IrExpr::String(format!("threw {label}")),
        };
        let message = IrExpr::Concat {
            left: Box::new(message),
            right: Box::new(IrExpr::String("\n".to_string())),
        };
        let arm_body = IrExpr::Let {
            name: "__test_reported".to_string(),
            value_ty: Type::Unit,
            value: Box::new(IrExpr::Platform {
                name: "io.write_stderr".to_string(),
                args: vec![message],
                ret: Type::Unit,
                throws: None,
            }),
            next: Box::new(IrExpr::Panic {
                message: Box::new(IrExpr::String("test failed".to_string())),
            }),
        };
        let wrapped = IrExpr::Try {
            body: Box::new(ir),
            arms: vec![IrArm {
                pattern: IrPattern::Wildcard {
                    binding: Some((error_var, error_ty)),
                },
                guard: None,
                body: arm_body,
            }],
            ty: ty.clone(),
            err_name: None,
        };
        (wrapped, ty)
    }

    /// The extern/intrinsic `name` resolves to from the current context (spec
    /// 0037), mirroring the type checker's `visible_extern` so both passes
    /// agree: an effect backing operation is visible only to sibling operations
    /// of its effect, any other extern only within its declaring module.
    fn visible_extern(&self, name: &str) -> Option<&ExternInfo> {
        let info = self.externs.get(name)?;
        // A Core Prelude intrinsic (spec 0021) is bare-visible from every module,
        // matching the type checker: the prelude is imported everywhere, so its
        // pure primitives cross module boundaries by bare name.
        if info.is_intrinsic && info.module.as_deref() == Some(crate::prelude::CORE_MODULE) {
            return Some(info);
        }
        let visible = match &info.effect_name {
            Some(effect) => self.current_effect.borrow().as_deref() == Some(effect.as_str()),
            None => {
                self.current_declared_module.borrow().as_deref() == info.module.as_deref()
                    // Host interface externs (spec 0026) are callable from any
                    // module that imports their host interface package.
                    || info
                        .module
                        .as_deref()
                        .is_some_and(|m| m.starts_with("host."))
                        && !info.is_intrinsic
            }
        };
        visible.then_some(info)
    }

    fn lower_call(
        &self,
        callee: &Expr,
        args: &[Expr],
        scope: &mut Scope,
        expected: Option<&Type>,
    ) -> (IrExpr, Type) {
        // Method-call (receiver) syntax (spec 0020): `recv.method(args)` on a
        // local value desugars to `method(recv, args)`, mirroring the checker.
        if let Expr::Path { segments, span } = callee
            && segments.len() == 2
            && scope.contains_key(&segments[0])
        {
            let receiver = Expr::Var(segments[0].clone(), span.clone());
            let method = Expr::Var(segments[1].clone(), span.clone());
            let mut method_args = Vec::with_capacity(args.len() + 1);
            method_args.push(receiver);
            method_args.extend(args.iter().cloned());
            return self.lower_call(&method, &method_args, scope, expected);
        }
        if let Expr::Var(name, _) = callee {
            // Bare `Some(x)` (spec 0042 O3): the present variant of the `option`
            // lang item, lowered like `Option::Some(x)`.
            if let Some(li) = &self.option_lang_item
                && *name == li.some
                && !scope.contains_key(name)
                && self.visible_extern(name).is_none()
                && matches!(
                    self.table.resolve_in(
                        std::slice::from_ref(name),
                        self.current_module.borrow().as_slice()
                    ),
                    Resolved::None | Resolved::BareImported(_)
                )
            {
                let enum_name = li.enum_name.clone();
                let (arg_ir, arg_ty) = self.lower_expr(&args[0], scope);
                let payload_tys = [self.apply(&arg_ty)];
                let def = self.enums[&enum_name]
                    .iter()
                    .find(|v| v.name == *name)
                    .expect("option lang item validated in type checking");
                let type_args = self.enum_type_args(&enum_name, def, &payload_tys);
                let ty = Type::Enum(enum_name.clone(), type_args);
                return (
                    IrExpr::EnumValue {
                        ty: ty.clone(),
                        variant: name.clone(),
                        tag: def.tag,
                        payload: vec![arg_ir],
                    },
                    ty,
                );
            }
            // A call to an `extern`/`intrinsic` declaration visible from the
            // current module/effect (spec 0037). A platform function (spec
            // 0013) lowers to a Platform node (a runtime call); an intrinsic
            // (spec 0021) lowers to an Intrinsic node (a native instruction
            // the backend inlines).
            if let Some(info) = self.visible_extern(name) {
                let is_intrinsic = info.is_intrinsic;
                let canonical = info.canonical.clone();
                let throws = info.throws.clone();
                let declared_params = info.params.clone();
                let declared_ret = info.ret.clone();
                let lowered: Vec<(IrExpr, Type)> =
                    args.iter().map(|arg| self.lower_expr(arg, scope)).collect();
                // Monomorphize a generic intrinsic's return type (spec 0021) from
                // the actual argument types, so `Type::Var` never reaches the IR.
                // A no-op for a concrete signature (empty subst).
                let mut subst = HashMap::new();
                for (declared, (_, actual)) in declared_params.iter().zip(lowered.iter()) {
                    infer_subst(declared, actual, &mut subst);
                }
                let ret = subst_type(&declared_ret, &subst);
                let args = lowered.into_iter().map(|(expr, _)| expr).collect();
                let node = if is_intrinsic {
                    IrExpr::Intrinsic {
                        name: name.clone(),
                        args,
                        ret: ret.clone(),
                    }
                } else {
                    IrExpr::Platform {
                        name: canonical,
                        args,
                        ret: ret.clone(),
                        throws,
                    }
                };
                return (node, ret);
            }
            // A call to a generic function (spec 0014).
            if !scope.contains_key(name)
                && let Resolved::One(entry) = self.table.resolve_in(
                    std::slice::from_ref(name),
                    self.current_module.borrow().as_slice(),
                )
                && entry.is_generic
            {
                return self.lower_generic_call(entry.index, args, scope);
            }
            // A bare trait method call (spec 0020), resolved after `FnTable` so
            // a same-module function still shadows it, mirroring the type
            // checker. Imported functions never bind bare names (spec 0037), so
            // a trait method also wins over a same-named import.
            if !scope.contains_key(name)
                && matches!(
                    self.table.resolve_in(
                        std::slice::from_ref(name),
                        self.current_module.borrow().as_slice()
                    ),
                    Resolved::None | Resolved::BareImported(_)
                )
                && let Some(candidates) = self.method_owners.get(name)
            {
                return self.lower_trait_call(candidates, name, args, scope, expected);
            }
        }
        // A `::` type-path call target (specs 0005/0018 R7): an enum variant
        // constructor, resolved through the enum type. The former
        // `Char::from_code` / `String::from_char` / `Array::*` builtins are now
        // bare intrinsics (spec 0021), lowered by the extern/intrinsic path above.
        if let Expr::TypePath { segments, .. } = callee
            && let [head, tail] = segments.as_slice()
        {
            // An enum variant constructor with a payload, e.g. `Color::Red(x)`.
            if let Some(variants) = self.enums.get(head)
                && let Some(def) = variants.iter().find(|v| v.name == *tail)
            {
                let lowered: Vec<(IrExpr, Type)> =
                    args.iter().map(|arg| self.lower_expr(arg, scope)).collect();
                // Infer the concrete type arguments from the payload (spec 0028).
                let payload_tys: Vec<Type> = lowered.iter().map(|(_, ty)| self.apply(ty)).collect();
                let args_ty = self.enum_type_args(head, def, &payload_tys);
                let payload = lowered.into_iter().map(|(expr, _)| expr).collect();
                let ty = Type::Enum(head.clone(), args_ty);
                return (
                    IrExpr::EnumValue {
                        ty: ty.clone(),
                        variant: tail.clone(),
                        tag: def.tag,
                        payload,
                    },
                    ty,
                );
            }
        }
        // A qualified `.` call target (spec 0018): a qualified trait method or a
        // (possibly generic) qualified function. A non-generic qualified function
        // falls through to the general path, where `lower_expr` on the path
        // yields its `FunctionRef`.
        if let Expr::Path { segments, .. } = callee {
            if let [head, tail] = segments.as_slice() {
                // A qualified trait method call `Trait.method(...)` (spec 0020).
                if self
                    .trait_methods
                    .contains_key(&(head.clone(), tail.clone()))
                {
                    return self.lower_trait_call(
                        std::slice::from_ref(head),
                        tail,
                        args,
                        scope,
                        expected,
                    );
                }
            }
            if let Resolved::One(entry) = self
                .table
                .resolve_in(segments, self.current_module.borrow().as_slice())
                && entry.is_generic
            {
                return self.lower_generic_call(entry.index, args, scope);
            }
        }
        let (callee, callee_ty) = self.lower_expr(callee, scope);
        let (ret, param_tys) = match callee_ty {
            Type::Function(function) => ((*function.ret).clone(), function.params.clone()),
            _ => (Type::Unit, Vec::new()),
        };
        // Each argument's expected type is the corresponding parameter type
        // (spec 0047), so a return-dispatched `empty()` in argument position
        // resolves here too.
        let ir_args = args
            .iter()
            .enumerate()
            .map(|(i, arg)| self.lower_expr_expected(arg, scope, param_tys.get(i)).0)
            .collect();
        (
            IrExpr::Call {
                callee: Box::new(callee),
                args: ir_args,
                ret: ret.clone(),
            },
            ret,
        )
    }

    /// Lowers a call to a generic function (spec 0014): infer its type arguments
    /// from the (now concrete) argument types, request the matching
    /// specialization, and call it by its mangled name. `template_index` is the
    /// generic function's index in `Program::functions`.
    fn lower_generic_call(
        &self,
        template_index: usize,
        args: &[Expr],
        scope: &mut Scope,
    ) -> (IrExpr, Type) {
        let template = &self.functions[template_index];
        let lowered: Vec<(IrExpr, Type)> =
            args.iter().map(|arg| self.lower_expr(arg, scope)).collect();
        let mut subst = HashMap::new();
        for (param, (_, actual)) in template.params.iter().zip(lowered.iter()) {
            infer_subst(&param.ty, actual, &mut subst);
        }
        let mangled = mangle(
            &template.module_path,
            &template.name,
            &template.type_params,
            &subst,
        );
        let sig = FunctionType {
            params: template
                .params
                .iter()
                .map(|param| subst_type(&param.ty, &subst))
                .collect(),
            ret: Box::new(subst_type(&template.ret, &subst)),
            throws: template
                .throws
                .as_ref()
                .map(|throws| Box::new(subst_type(throws, &subst))),
            effects: template.effects.clone(),
        };
        self.request_specialization(&mangled, template_index, subst);
        let ret = (*sig.ret).clone();
        (
            IrExpr::Call {
                callee: Box::new(IrExpr::FunctionRef { name: mangled, sig }),
                args: lowered.into_iter().map(|(expr, _)| expr).collect(),
                ret: ret.clone(),
            },
            ret,
        )
    }

    fn lower_arm(
        &self,
        arm: &MatchArm,
        scrutinee_ty: &Type,
        variants: &[VariantDef],
        scope: &Scope,
        expected: Option<&Type>,
    ) -> IrArm {
        let mut arm_scope = scope.clone();
        let pattern = self.lower_pattern(&arm.pattern, scrutinee_ty, variants, &mut arm_scope);
        let guard = arm
            .guard
            .as_ref()
            .map(|guard| self.lower_expr(guard, &mut arm_scope).0);
        // The arm produces the match's value, so it inherits the expected type
        // (spec 0047): `Nil -> empty()` resolves here.
        let body = self
            .lower_expr_expected(&arm.body, &mut arm_scope, expected)
            .0;
        IrArm {
            pattern,
            guard,
            body,
        }
    }

    fn lower_pattern(
        &self,
        pattern: &Pattern,
        scrutinee_ty: &Type,
        variants: &[VariantDef],
        scope: &mut Scope,
    ) -> IrPattern {
        match pattern {
            Pattern::Wildcard(_) => IrPattern::Wildcard { binding: None },
            Pattern::Binding { name, .. } => {
                scope.insert(name.clone(), scrutinee_ty.clone());
                IrPattern::Wildcard {
                    binding: Some((name.clone(), scrutinee_ty.clone())),
                }
            }
            Pattern::Variant {
                enum_name,
                variant,
                fields,
                ..
            } => {
                // A qualified pattern names its enum directly; otherwise use the
                // scrutinee's variants.
                let owned;
                let resolved: &[VariantDef] = match enum_name {
                    Some(name) => {
                        // Reuse the scrutinee's type arguments when the qualified
                        // enum is the scrutinee's own type (spec 0028), so bindings
                        // stay concrete.
                        let args = match scrutinee_ty {
                            Type::Enum(sname, sargs) if sname == name => sargs.clone(),
                            _ => Vec::new(),
                        };
                        owned = self.variants_of(&Type::Enum(name.clone(), args));
                        &owned
                    }
                    None => variants,
                };
                let info = resolved.iter().find(|v| v.name == *variant);
                let tag = info.map_or(0, |v| v.tag);
                let field_tys = info.map(|v| v.fields.clone()).unwrap_or_default();
                let bindings = fields
                    .iter()
                    .enumerate()
                    .map(|(index, binding)| match binding {
                        FieldBinding::Name(name) => {
                            let ty = field_tys.get(index).cloned().unwrap_or(Type::Unit);
                            scope.insert(name.clone(), ty.clone());
                            Some((name.clone(), ty))
                        }
                        FieldBinding::Ignore => None,
                    })
                    .collect();
                IrPattern::Variant {
                    variant: variant.clone(),
                    tag,
                    bindings,
                }
            }
        }
    }

    /// The variants a matched value of `ty` can take (spec 0005). `Option<T>`
    /// is the built-in `Some(T)`/`None` enum with tags 0 and 1.
    /// The concrete type arguments of a constructed enum value (spec 0028),
    /// inferred from the payload types like the type checker does. Parameters the
    /// payload does not pin (including every one of a payload-less variant such
    /// as `Nil`) are `Never`, mirroring `None : Option<Never>`.
    fn enum_type_args(
        &self,
        enum_name: &str,
        variant: &VariantDef,
        payload_tys: &[Type],
    ) -> Vec<Type> {
        let Some(params) = self.enum_type_params.get(enum_name) else {
            return Vec::new();
        };
        if params.is_empty() {
            return Vec::new();
        }
        let mut subst = HashMap::new();
        for (field, actual) in variant.fields.iter().zip(payload_tys) {
            infer_subst(field, actual, &mut subst);
        }
        params
            .iter()
            .map(|param| subst.get(param).cloned().unwrap_or(Type::Never))
            .collect()
    }

    fn variants_of(&self, ty: &Type) -> Vec<VariantDef> {
        match ty {
            Type::Enum(name, args) => {
                let Some(variants) = self.enums.get(name) else {
                    return Vec::new();
                };
                // Substitute the concrete type arguments into each variant's
                // field types (spec 0028) so pattern bindings get concrete types.
                let params = self.enum_type_params.get(name).cloned().unwrap_or_default();
                let subst: HashMap<String, Type> =
                    params.into_iter().zip(args.iter().cloned()).collect();
                variants
                    .iter()
                    .map(|v| VariantDef {
                        name: v.name.clone(),
                        tag: v.tag,
                        fields: v.fields.iter().map(|f| subst_type(f, &subst)).collect(),
                    })
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// The variables a lambda captures from its enclosing runtime scope, in
    /// first-occurrence order. Top-level functions and platform functions are
    /// not in `scope`, so they are never captured.
    fn lambda_captures(
        &self,
        params: &[crate::ast::Param],
        body: &Block,
        scope: &Scope,
    ) -> Vec<IrCapture> {
        let bound: HashSet<String> = params.iter().map(|param| param.name.clone()).collect();
        let mut free = Vec::new();
        free_vars_block(&body.items, &bound, &mut free);
        free.into_iter()
            .filter_map(|name| {
                scope.get(&name).map(|ty| IrCapture {
                    name,
                    ty: ty.clone(),
                })
            })
            .collect()
    }
}

/// The error type a single lowered site can raise: a `throw`'s value type or a
/// throwing call's declared error. Unlike [`body_error_ty`] this does not walk
/// into subexpressions — nested sites in a `@test` body wrap themselves (spec
/// 0040 T3), so only the node itself is inspected.
fn site_error_ty(ir: &IrExpr) -> Option<Type> {
    match ir {
        IrExpr::Throw { value } => Some(value.ty()),
        IrExpr::Call { callee, .. } => match callee.ty() {
            Type::Function(function) => function.throws.map(|throws| *throws),
            _ => None,
        },
        _ => None,
    }
}

/// A short display label for a thrown error's type in the test-failure report
/// (spec 0040 C7), e.g. `AssertError`.
fn error_type_label(ty: &Type) -> String {
    match ty {
        Type::Enum(name, _) => name.clone(),
        other => format!("{other:?}"),
    }
}

/// The error type a `try` body can raise, found from the first `throw` or
/// throwing call in it. The type checker guarantees a single error type, so the
/// first one found is representative — it is used to resolve `catch` arm tags.
fn body_error_ty(ir: &IrExpr) -> Option<Type> {
    match ir {
        IrExpr::Throw { value } => Some(value.ty()),
        IrExpr::Call { callee, .. } => match callee.ty() {
            Type::Function(function) => function.throws.map(|throws| *throws),
            _ => None,
        },
        IrExpr::Question { value, .. } => body_error_ty(value),
        IrExpr::Let { value, next, .. } => body_error_ty(value).or_else(|| body_error_ty(next)),
        IrExpr::Binary { left, right, .. } => body_error_ty(left).or_else(|| body_error_ty(right)),
        IrExpr::Match {
            scrutinee, arms, ..
        } => body_error_ty(scrutinee)
            .or_else(|| arms.iter().find_map(|arm| body_error_ty(&arm.body))),
        IrExpr::Array { elems, .. } => elems.iter().find_map(body_error_ty),
        IrExpr::EnumValue { payload, .. } => payload.iter().find_map(body_error_ty),
        _ => None,
    }
}

/// Picks the representative type of a set of arm bodies, preferring a concrete
/// type over `Never` (which a `panic`/`throw`-only arm yields).
fn pick_ty(types: impl Iterator<Item = Type>) -> Type {
    let mut result = Type::Never;
    for ty in types {
        if !matches!(ty, Type::Never) {
            return ty;
        }
        result = ty;
    }
    result
}

/// Boolean negation of a `Bool` expression. There is no `!` node in the IR, so
/// `!cond` becomes `if cond { false } else { true }` (spec 0027).
fn bool_not(cond: IrExpr) -> IrExpr {
    IrExpr::If {
        cond: Box::new(cond),
        then: Box::new(IrExpr::Bool(false)),
        els: Box::new(IrExpr::Bool(true)),
        ty: Type::Bool,
    }
}

fn free_vars_block(items: &[BlockItem], bound: &HashSet<String>, out: &mut Vec<String>) {
    let mut bound = bound.clone();
    for item in items {
        match item {
            BlockItem::Let { name, value, .. } => {
                free_vars_expr(value, &bound, out);
                bound.insert(name.clone());
            }
            BlockItem::Expr(expr) => free_vars_expr(expr, &bound, out),
        }
    }
}

fn free_vars_expr(expr: &Expr, bound: &HashSet<String>, out: &mut Vec<String>) {
    match expr {
        Expr::Var(name, _) => {
            if !bound.contains(name) && !out.contains(name) {
                out.push(name.clone());
            }
        }
        Expr::Array(elements, _) => {
            for element in elements {
                free_vars_expr(element, bound, out);
            }
        }
        Expr::Call { callee, args, .. } => {
            free_vars_expr(callee, bound, out);
            for arg in args {
                free_vars_expr(arg, bound, out);
            }
        }
        Expr::Binary { left, right, .. } => {
            free_vars_expr(left, bound, out);
            free_vars_expr(right, bound, out);
        }
        Expr::Fn { params, body, .. } => {
            let mut inner = bound.clone();
            for param in params {
                inner.insert(param.name.clone());
            }
            free_vars_block(&body.items, &inner, out);
        }
        Expr::Block(block) => free_vars_block(&block.items, bound, out),
        Expr::If {
            cond, then, els, ..
        } => {
            free_vars_expr(cond, bound, out);
            free_vars_block(&then.items, bound, out);
            free_vars_block(&els.items, bound, out);
        }
        Expr::Throw { value, .. } | Expr::Question { value, .. } => {
            free_vars_expr(value, bound, out)
        }
        Expr::Panic { message, .. } => free_vars_expr(message, bound, out),
        // A path's head may be a local value (a receiver call or record field
        // access, specs 0020/0006); the later segments never are. When it is a
        // call target its arguments live in the wrapping `Call`, handled above.
        Expr::Path { segments, .. } => {
            if let Some(head) = segments.first()
                && !bound.contains(head)
                && !out.contains(head)
            {
                out.push(head.clone());
            }
        }
        Expr::TypePath { .. } => {}
        Expr::RecordLiteral { fields, .. } => {
            for (_, _, value) in fields {
                free_vars_expr(value, bound, out);
            }
        }
        Expr::Field { target, .. } => free_vars_expr(target, bound, out),
        Expr::Match {
            scrutinee, arms, ..
        } => {
            free_vars_expr(scrutinee, bound, out);
            for arm in arms {
                free_vars_arm(arm, bound, out);
            }
        }
        Expr::Try { body, arms, .. } => {
            free_vars_block(&body.items, bound, out);
            for arm in arms {
                free_vars_arm(arm, bound, out);
            }
        }
        Expr::Int(_, _)
        | Expr::Float(_, _)
        | Expr::Bool(_, _)
        | Expr::String(_, _)
        | Expr::Char(_, _)
        | Expr::Unit(_) => {}
    }
}

fn free_vars_arm(arm: &MatchArm, bound: &HashSet<String>, out: &mut Vec<String>) {
    let mut inner = bound.clone();
    pattern_bindings(&arm.pattern, &mut inner);
    if let Some(guard) = &arm.guard {
        free_vars_expr(guard, &inner, out);
    }
    free_vars_expr(&arm.body, &inner, out);
}

fn pattern_bindings(pattern: &Pattern, bound: &mut HashSet<String>) {
    match pattern {
        Pattern::Wildcard(_) => {}
        Pattern::Binding { name, .. } => {
            bound.insert(name.clone());
        }
        Pattern::Variant { fields, .. } => {
            for field in fields {
                if let FieldBinding::Name(name) = field {
                    bound.insert(name.clone());
                }
            }
        }
    }
}

/// Binds a generic function's type parameters by matching a declared parameter
/// type (which may contain type variables) against the concrete `actual` type of
/// the argument (spec 0014). The type checker has already validated the call, so
/// this is total: structural mismatches cannot occur here.
fn infer_subst(declared: &Type, actual: &Type, subst: &mut HashMap<String, Type>) {
    match (declared, actual) {
        (Type::Var(name), _) => {
            // Prefer a concrete binding over `Never` (from `throw`/`None`).
            match subst.get(name) {
                Some(bound) if *bound != Type::Never => {}
                _ => {
                    subst.insert(name.clone(), actual.clone());
                }
            }
        }
        (Type::Array(d), Type::Array(a)) => infer_subst(d, a, subst),
        (Type::Enum(dn, dargs), Type::Enum(an, aargs))
            if dn == an && dargs.len() == aargs.len() =>
        {
            for (d, a) in dargs.iter().zip(aargs.iter()) {
                infer_subst(d, a, subst);
            }
        }
        (Type::Function(d), Type::Function(a)) if d.params.len() == a.params.len() => {
            for (dp, ap) in d.params.iter().zip(a.params.iter()) {
                infer_subst(dp, ap, subst);
            }
            infer_subst(&d.ret, &a.ret, subst);
        }
        _ => {}
    }
}

/// The mangled name of a specialization, e.g. root `identity` at `T = Int`
/// becomes `identity__Int`, and imported `std.list.map` at `T = U = Int` becomes
/// `std__list__map__Int__Int`. The module path is included so same-named generic
/// functions from different modules (e.g. `list.map` and `option.map`) get
/// distinct specializations instead of colliding on one emit name. Deterministic
/// and identifier-safe so backends can use it verbatim; type parameters are
/// appended in declaration order.
fn mangle(
    module_path: &[String],
    name: &str,
    type_params: &[String],
    subst: &HashMap<String, Type>,
) -> String {
    let mut mangled = String::new();
    for segment in module_path {
        mangled.push_str(segment);
        mangled.push_str("__");
    }
    mangled.push_str(name);
    for type_param in type_params {
        mangled.push_str("__");
        mangled.push_str(&mangle_type(subst.get(type_param).unwrap_or(&Type::Unit)));
    }
    mangled
}

/// The mangled name of a specialized impl method (spec 0020), e.g. `Add.add` for
/// `Int` becomes `Add__Int__add`. Distinct from the generic mangle
/// (`identity__Int`) and the path mangle (`std__int__to_string`), so the three
/// naming schemes never collide.
fn mangle_impl_method(trait_name: &str, self_ty: &Type, method: &str) -> String {
    format!("{trait_name}__{}__{method}", mangle_type(self_ty))
}

/// An identifier-safe encoding of a concrete type for name mangling.
fn mangle_type(ty: &Type) -> String {
    match ty {
        Type::Unit => "Unit".to_string(),
        Type::Bool => "Bool".to_string(),
        Type::Int => "Int".to_string(),
        Type::Float => "Float".to_string(),
        Type::String => "String".to_string(),
        Type::Char => "Char".to_string(),
        Type::Array(element) => format!("Array_{}_", mangle_type(element)),
        Type::Record => "Record".to_string(),
        Type::Enum(name, args) if args.is_empty() => name.clone(),
        // A generic enum instance mangles by name and arguments, so distinct
        // instantiations get distinct specialization names (spec 0028/0014).
        // `Option<Int>` mangles `Option_Int_`, as the built-in once did.
        Type::Enum(name, args) => format!(
            "{name}_{}_",
            args.iter().map(mangle_type).collect::<Vec<_>>().join("_")
        ),
        Type::Never => "Never".to_string(),
        Type::Function(function) => {
            let params = function
                .params
                .iter()
                .map(mangle_type)
                .collect::<Vec<_>>()
                .join("_");
            format!("Fn_{params}_to_{}_", mangle_type(&function.ret))
        }
        Type::OpaqueFunction => "Fn".to_string(),
        // A type variable cannot survive substitution at a concrete call site.
        Type::Var(name) => name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_program;
    use crate::typecheck;

    fn lower_source(source: &str) -> IrProgram {
        let (mut program, errors) = parse_program("test", source);
        assert!(errors.is_empty(), "parse: {errors:?}");
        // Mirror the driver: operators resolve through the embedded Core Prelude
        // (spec 0021), and defaulted trait methods are filled in (spec 0020).
        crate::driver::merge_prelude(&mut program).expect("prelude");
        typecheck::expand_trait_defaults(&mut program);
        let (typed, errors) =
            typecheck::check(&program, true, &emela_codegen::platform_interface());
        assert!(errors.is_empty(), "typecheck: {errors:?}");
        lower(&program, &typed)
    }

    fn main_body(ir: &IrProgram) -> &IrExpr {
        &ir.functions
            .iter()
            .find(|function| function.name == "main")
            .expect("main")
            .body
    }

    // Walk to the first `Fn` literal in an expression tree.
    fn first_lambda(expr: &IrExpr) -> Option<&IrExpr> {
        match expr {
            IrExpr::Fn { .. } => Some(expr),
            IrExpr::Let { value, next, .. } => first_lambda(value).or_else(|| first_lambda(next)),
            IrExpr::Call { callee, args, .. } => {
                first_lambda(callee).or_else(|| args.iter().find_map(first_lambda))
            }
            IrExpr::Binary { left, right, .. } => {
                first_lambda(left).or_else(|| first_lambda(right))
            }
            IrExpr::Array { elems, .. } => elems.iter().find_map(first_lambda),
            _ => None,
        }
    }

    #[test]
    fn lambda_captures_enclosing_binding() {
        let ir = lower_source(
            "fn make_adder(n: Int) -> (Int) -> Int {\n  fn (x: Int) -> Int { x + n }\n}\nfn main() -> Int { let a = make_adder(1) a(41) }\n",
        );
        let adder = ir
            .functions
            .iter()
            .find(|function| function.name == "make_adder")
            .expect("make_adder");
        let lambda = first_lambda(&adder.body).expect("lambda");
        let IrExpr::Fn { captures, .. } = lambda else {
            panic!("expected Fn");
        };
        assert_eq!(captures.len(), 1);
        assert_eq!(captures[0].name, "n");
        assert_eq!(captures[0].ty, Type::Int);
    }

    #[test]
    fn monomorphizes_generic_call() {
        let ir =
            lower_source("fn identity<T>(x: T) -> T { x }\nfn main() -> Int { identity(42) }\n");
        let names: Vec<&str> = ir.functions.iter().map(|f| f.name.as_str()).collect();
        // The call is specialized and the generic template is not emitted.
        assert!(names.contains(&"identity__Int"), "names: {names:?}");
        assert!(!names.contains(&"identity"), "names: {names:?}");
        // The specialization is fully concrete.
        let spec = ir
            .functions
            .iter()
            .find(|function| function.name == "identity__Int")
            .expect("identity__Int");
        assert_eq!(spec.ret, Type::Int);
        assert_eq!(spec.params[0].ty, Type::Int);
    }

    #[test]
    fn top_level_functions_are_not_captured() {
        let ir = lower_source(
            "fn helper(x: Int) -> Int { x }\nfn main() -> Int {\n  let k = 2\n  let f = fn (x: Int) -> Int { helper(x) + k }\n  f(40)\n}\n",
        );
        let lambda = first_lambda(main_body(&ir)).expect("lambda");
        let IrExpr::Fn { captures, .. } = lambda else {
            panic!("expected Fn");
        };
        let names: Vec<&str> = captures.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["k"]);
    }

    #[test]
    fn preserves_non_tail_block_expressions_in_source_order() {
        let ir = lower_source(
            "fn mark(n: Int) -> Int { n }\nfn main() -> Int {\n  mark(1)\n  let a = 2\n  mark(2)\n  a\n}\n",
        );
        let IrExpr::Let {
            name: stmt0,
            value: first,
            next,
            ..
        } = main_body(&ir)
        else {
            panic!("expected leading statement let");
        };
        assert!(stmt0.starts_with("$stmt"));
        let IrExpr::Call { callee, args, ret } = first.as_ref() else {
            panic!("expected first call");
        };
        let IrExpr::FunctionRef { name, .. } = callee.as_ref() else {
            panic!("expected first direct call");
        };
        assert_eq!(name, "mark");
        assert_eq!(ret, &Type::Int);
        assert!(matches!(args.as_slice(), [IrExpr::Int(1)]));

        let IrExpr::Let {
            name, value, next, ..
        } = next.as_ref()
        else {
            panic!("expected user let binding");
        };
        assert_eq!(name, "a");
        assert!(matches!(value.as_ref(), IrExpr::Int(2)));

        let IrExpr::Let {
            name: stmt1,
            value: second,
            next,
            ..
        } = next.as_ref()
        else {
            panic!("expected second statement let");
        };
        assert!(stmt1.starts_with("$stmt"));
        let IrExpr::Call { callee, args, ret } = second.as_ref() else {
            panic!("expected second call");
        };
        let IrExpr::FunctionRef { name, .. } = callee.as_ref() else {
            panic!("expected second direct call");
        };
        assert_eq!(name, "mark");
        assert_eq!(ret, &Type::Int);
        assert!(matches!(args.as_slice(), [IrExpr::Int(2)]));

        let IrExpr::Var { name, ty } = next.as_ref() else {
            panic!("expected tail expression");
        };
        assert_eq!(name, "a");
        assert_eq!(ty, &Type::Int);
    }

    #[test]
    fn preserves_non_tail_never_expressions() {
        let ir = lower_source("fn main() -> Unit {\n  panic(\"boom\")\n  ()\n}\n");
        let IrExpr::Let { value, next, .. } = main_body(&ir) else {
            panic!("expected statement let");
        };
        let IrExpr::Panic { message } = value.as_ref() else {
            panic!("expected panic statement");
        };
        assert!(matches!(message.as_ref(), IrExpr::String(value) if value == "boom"));
        assert!(matches!(next.as_ref(), IrExpr::Unit));
    }

    #[test]
    fn comparison_temp_names_do_not_shadow_user_bindings() {
        let ir = lower_source("fn main() -> Int {\n  let cmp0 = 41\n  cmp0 > 0\n  cmp0\n}\n");
        let IrExpr::Let {
            name, value, next, ..
        } = main_body(&ir)
        else {
            panic!("expected user let binding");
        };
        assert_eq!(name, "cmp0");
        assert!(matches!(value.as_ref(), IrExpr::Int(41)));

        let IrExpr::Let {
            name: stmt, next, ..
        } = next.as_ref()
        else {
            panic!("expected comparison statement temp");
        };
        assert!(stmt.starts_with("$stmt"));

        let IrExpr::Var { name, ty } = next.as_ref() else {
            panic!("expected tail variable");
        };
        assert_eq!(name, "cmp0");
        assert_eq!(ty, &Type::Int);
    }
}
