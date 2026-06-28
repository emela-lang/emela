use std::collections::{BTreeSet, HashMap};

use serde::Serialize;

use crate::ast::{
    BinaryOp, Block, BlockItem, Capability, EnumDecl, Expr, Function,
    FunctionType as AstFunctionType, ImportOrigin, MatchArm, Pattern, PrimType, Program,
    StructDecl, TopLevelItem, Type,
};
use crate::error::{Diagnostic, Error, Result, Span};
use crate::platform::PlatformSpec;

#[derive(Debug, Clone)]
struct TypeSlot {
    parent: usize,
    value: Option<Type>,
}

#[derive(Debug, Clone)]
struct FunctionType {
    type_params: Vec<String>,
    param_types: Vec<Type>,
    ret_type: Type,
    params: Vec<usize>,
    ret: usize,
    effectful: bool,
    declared_capabilities: Option<BTreeSet<Capability>>,
}

#[derive(Debug, Clone)]
struct VariantInfo {
    enum_name: String,
    enum_type_params: Vec<String>,
    payload: Option<Type>,
}

#[derive(Debug, Clone)]
struct ExprInfo {
    ty: usize,
    effectful: bool,
    capabilities: BTreeSet<Capability>,
    span: Option<Span>,
}

pub(crate) struct TypeChecker<'a> {
    program: &'a Program,
    platform: &'a PlatformSpec,
    mode: CheckMode,
    types: Vec<TypeSlot>,
    structs: HashMap<String, &'a StructDecl>,
    enums: HashMap<String, &'a EnumDecl>,
    variants: HashMap<String, VariantInfo>,
    functions: HashMap<String, FunctionType>,
    rigid_type_params: BTreeSet<String>,
    diagnostic_context: Option<(Span, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckMode {
    Executable,
    Library,
}

impl<'a> TypeChecker<'a> {
    #[cfg(test)]
    pub(crate) fn new(program: &'a Program, platform: &'a PlatformSpec) -> Self {
        Self::new_with_mode(program, platform, CheckMode::Executable)
    }

    pub(crate) fn new_with_mode(
        program: &'a Program,
        platform: &'a PlatformSpec,
        mode: CheckMode,
    ) -> Self {
        Self {
            program,
            platform,
            mode,
            types: Vec::new(),
            structs: HashMap::new(),
            enums: HashMap::new(),
            variants: HashMap::new(),
            functions: HashMap::new(),
            rigid_type_params: BTreeSet::new(),
            diagnostic_context: None,
        }
    }

    pub(crate) fn check(mut self) -> Result<TypedProgram> {
        self.register_types()?;
        self.register_imports()?;
        self.register_functions()?;
        if self.mode == CheckMode::Executable {
            self.check_main()?;
        }

        let functions = self.program.functions();
        let mut function_capabilities = HashMap::new();
        for function in &functions {
            let signature = self
                .functions
                .get(&function.name)
                .cloned()
                .ok_or_else(|| Error::new("internal type checker error"))?;
            let previous_rigid_type_params = std::mem::replace(
                &mut self.rigid_type_params,
                function.type_params.iter().cloned().collect(),
            );
            let mut scope = HashMap::new();
            for (param, ty) in function.params.iter().zip(signature.params.iter()) {
                if scope.insert(param.name.clone(), *ty).is_some() {
                    return Err(Error::diagnostic(
                        Diagnostic::new("Duplicate parameter")
                            .label(
                                param.name_span.clone(),
                                format!(
                                    "parameter `{}` is already defined in function `{}`.",
                                    param.name, function.name
                                ),
                            )
                            .help("Rename one of the parameters so each name is unique."),
                    ));
                }
            }

            let body = self.check_block(&function.body, &mut scope)?;
            let return_mismatch_span =
                self.return_mismatch_span(function, signature.ret, body.ty, body.span.as_ref());
            self.unify_at(
                signature.ret,
                body.ty,
                return_mismatch_span,
                format!("This is returned from `{}`.", function.name),
            )?;
            self.rigid_type_params = previous_rigid_type_params;

            if !signature.effectful && body.effectful {
                return Err(Error::diagnostic(
                    Diagnostic::new("Unhandled effects")
                        .label(
                            body.span.clone().unwrap_or_else(|| self.placeholder_span()),
                            format!("pure function `{}` cannot contain unhandled effects.", function.name),
                        )
                        .help("Rename the function with `!`, or move the effectful call out of this function."),
                ));
            }

            if let Some(declared) = &signature.declared_capabilities {
                if !body.capabilities.is_subset(declared) {
                    let missing = body
                        .capabilities
                        .difference(declared)
                        .map(|capability| format!("{capability:?}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(Error::diagnostic(
                        Diagnostic::new("Capability is not declared")
                            .label(
                                body.span.clone().unwrap_or_else(|| self.placeholder_span()),
                                format!(
                                    "function `{}` uses capability outside #[requires(...)]: {missing}",
                                    function.name
                                ),
                            )
                            .help("Add the missing capability to `#[requires(...)]`, or remove the call that needs it."),
                    ));
                }
            }

            let function_caps = signature
                .declared_capabilities
                .clone()
                .unwrap_or(body.capabilities);
            function_capabilities.insert(function.name.clone(), function_caps);
        }

        if self.mode == CheckMode::Executable {
            self.check_runtime_boundary(&function_capabilities)?;
        }

        let mut typed_functions = Vec::new();
        for function in &functions {
            let signature = self
                .functions
                .get(&function.name)
                .cloned()
                .ok_or_else(|| Error::new("internal type checker error"))?;
            let params = signature
                .params
                .iter()
                .map(|id| self.resolve_known(*id, &format!("parameter in `{}`", function.name)))
                .collect::<Result<Vec<_>>>()?;
            let ret = self.resolve_known(
                signature.ret,
                &format!("return type of `{}`", function.name),
            )?;
            typed_functions.push(TypedFunction {
                name: function.name.clone(),
                params,
                ret,
                effectful: signature.effectful,
                capabilities: function_capabilities
                    .remove(&function.name)
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
            });
        }

        Ok(TypedProgram {
            functions: typed_functions,
        })
    }

    fn register_types(&mut self) -> Result<()> {
        self.register_builtin_types();
        for item in &self.program.items {
            match item {
                TopLevelItem::Struct(decl) => {
                    if self.structs.contains_key(&decl.name) || self.enums.contains_key(&decl.name)
                    {
                        return Err(Error::diagnostic(
                            Diagnostic::new("Duplicate type")
                                .label(
                                    decl.name_span.clone(),
                                    format!("type `{}` is already defined.", decl.name),
                                )
                                .help("Rename this type or remove the earlier definition."),
                        ));
                    }
                    self.structs.insert(decl.name.clone(), decl);
                }
                TopLevelItem::Enum(decl) => {
                    if decl.variants.is_empty() {
                        return Err(Error::diagnostic(
                            Diagnostic::new("Enum has no variants")
                                .label(
                                    decl.name_span.clone(),
                                    format!(
                                        "enum `{}` must declare at least one variant.",
                                        decl.name
                                    ),
                                )
                                .help("Add a variant inside the enum body."),
                        ));
                    }
                    if self.structs.contains_key(&decl.name) || self.enums.contains_key(&decl.name)
                    {
                        return Err(Error::diagnostic(
                            Diagnostic::new("Duplicate type")
                                .label(
                                    decl.name_span.clone(),
                                    format!("type `{}` is already defined.", decl.name),
                                )
                                .help("Rename this type or remove the earlier definition."),
                        ));
                    }
                    for variant in &decl.variants {
                        if self.variants.contains_key(&variant.name) {
                            return Err(Error::diagnostic(
                                Diagnostic::new("Duplicate enum variant")
                                    .label(
                                        variant.name_span.clone(),
                                        format!("enum variant `{}` is already defined.", variant.name),
                                    )
                                    .help("Variant names are global right now; choose a unique variant name."),
                            ));
                        }
                        self.variants.insert(
                            variant.name.clone(),
                            VariantInfo {
                                enum_name: decl.name.clone(),
                                enum_type_params: decl.type_params.clone(),
                                payload: variant.payload.as_ref().map(|payload| {
                                    normalize_type_params(payload, &decl.type_params)
                                }),
                            },
                        );
                    }
                    self.enums.insert(decl.name.clone(), decl);
                }
                TopLevelItem::Function(_) | TopLevelItem::Import(_) => {}
            }
        }

        for decl in self.structs.values() {
            let mut seen = BTreeSet::new();
            for param in &decl.type_params {
                if !seen.insert(param) {
                    return Err(Error::diagnostic(
                        Diagnostic::new("Duplicate type parameter")
                            .label(
                                decl.name_span.clone(),
                                format!(
                                    "duplicate type parameter `{param}` in struct `{}`.",
                                    decl.name
                                ),
                            )
                            .help("Remove the duplicate parameter from the `<...>` list."),
                    ));
                }
            }
            self.validate_type_in_scope_at(&decl.field.ty, &decl.type_params, &decl.field.ty_span)?;
        }
        for decl in self.enums.values() {
            let mut seen = BTreeSet::new();
            for param in &decl.type_params {
                if !seen.insert(param) {
                    return Err(Error::diagnostic(
                        Diagnostic::new("Duplicate type parameter")
                            .label(
                                decl.name_span.clone(),
                                format!(
                                    "duplicate type parameter `{param}` in enum `{}`.",
                                    decl.name
                                ),
                            )
                            .help("Remove the duplicate parameter from the `<...>` list."),
                    ));
                }
            }
            for variant in &decl.variants {
                if let (Some(payload), Some(span)) = (&variant.payload, &variant.payload_span) {
                    self.validate_type_in_scope_at(payload, &decl.type_params, span)?;
                }
            }
        }
        Ok(())
    }

    fn register_builtin_types(&mut self) {
        self.variants.insert(
            "Ok".to_string(),
            VariantInfo {
                enum_name: "Result".to_string(),
                enum_type_params: vec!["T".to_string(), "E".to_string()],
                payload: Some(Type::GenericParam("T".to_string())),
            },
        );
        self.variants.insert(
            "Err".to_string(),
            VariantInfo {
                enum_name: "Result".to_string(),
                enum_type_params: vec!["T".to_string(), "E".to_string()],
                payload: Some(Type::GenericParam("E".to_string())),
            },
        );
        for name in [
            "Unsupported",
            "Unavailable",
            "Interrupted",
            "InvalidUtf8",
            "Unknown",
        ] {
            self.variants.insert(
                name.to_string(),
                VariantInfo {
                    enum_name: "PlatformError".to_string(),
                    enum_type_params: Vec::new(),
                    payload: None,
                },
            );
        }
    }

    fn register_imports(&mut self) -> Result<()> {
        for item in &self.program.items {
            let TopLevelItem::Import(import) = item else {
                continue;
            };
            if import.origin == ImportOrigin::User
                && import
                    .path
                    .first()
                    .is_some_and(|package| package == "platform")
            {
                return Err(Error::diagnostic(
                    Diagnostic::new("Platform import is private")
                        .label(
                            import.span.clone(),
                            format!(
                                "platform import `{}` is only available to stdlib.",
                                format_import_path(&import.path, &import.name)
                            ),
                        )
                        .help(
                            "Use a public stdlib function instead of importing platform internals.",
                        ),
                ));
            }
            if self.functions.contains_key(&import.name) {
                return Err(Error::diagnostic(
                    Diagnostic::new("Duplicate imported function")
                        .label(
                            import.span.clone(),
                            format!("duplicate imported function `{}`.", import.name),
                        )
                        .help("Remove one import, or import a different function name."),
                ));
            }
            let function = self
                .platform
                .externs
                .resolve_import(&import.path, &import.name)
                .ok_or_else(|| {
                    Error::diagnostic(
                        Diagnostic::new("Unknown external import")
                            .label(
                                import.span.clone(),
                                format!(
                                    "unknown external import `{}`.",
                                    format_import_path(&import.path, &import.name)
                                ),
                            )
                            .help("Check the import path and the selected backend/platform."),
                    )
                })?;
            let params = function
                .params
                .iter()
                .map(|ty| self.known(ty.clone()))
                .collect();
            let ret = self.known(function.ret.clone());
            self.functions.insert(
                import.name.clone(),
                FunctionType {
                    type_params: Vec::new(),
                    param_types: function.params.clone(),
                    ret_type: function.ret.clone(),
                    params,
                    ret,
                    effectful: function.effectful,
                    declared_capabilities: Some(function.capabilities.iter().copied().collect()),
                },
            );
        }
        Ok(())
    }

    fn register_functions(&mut self) -> Result<()> {
        for function in self.program.functions() {
            if self.functions.contains_key(&function.name) {
                return Err(Error::diagnostic(
                    Diagnostic::new("Duplicate function")
                        .label(
                            function.name_span.clone(),
                            format!("function `{}` is already defined.", function.name),
                        )
                        .help("Rename this function or remove the earlier definition."),
                ));
            }

            let declared_capabilities = function
                .requires
                .as_ref()
                .map(|capabilities| capabilities.iter().copied().collect::<BTreeSet<_>>());
            let has_capabilities = declared_capabilities
                .as_ref()
                .is_some_and(|capabilities| !capabilities.is_empty());
            let effectful = function.name.ends_with('!');
            if has_capabilities && !effectful {
                return Err(Error::diagnostic(
                    Diagnostic::new("Missing effect marker")
                        .label(
                            function.name_span.clone(),
                            format!(
                                "function `{}` requires platform capabilities and must be marked with !.",
                                function.name
                            ),
                        )
                        .help("Rename the function with a trailing `!`, for example `main!`."),
                ));
            }
            let mut seen_type_params = BTreeSet::new();
            for param in &function.type_params {
                if !seen_type_params.insert(param) {
                    return Err(Error::diagnostic(
                        Diagnostic::new("Duplicate type parameter")
                            .label(
                                function.name_span.clone(),
                                format!(
                                    "duplicate type parameter `{param}` in function `{}`.",
                                    function.name
                                ),
                            )
                            .help("Remove the duplicate parameter from the `<...>` list."),
                    ));
                }
            }

            let param_types = function
                .params
                .iter()
                .map(|param| match (&param.ty, &param.ty_span) {
                    (Some(ty), Some(span)) => {
                        self.validate_type_in_scope_at(ty, &function.type_params, span)?;
                        Ok(normalize_type_params(ty, &function.type_params))
                    }
                    (None, _) => Err(Error::diagnostic(
                        Diagnostic::new("Missing type annotation")
                            .label(
                                param.name_span.clone(),
                                format!(
                                    "parameter `{}` in function `{}` must have a type annotation.",
                                    param.name, function.name
                                ),
                            )
                            .help(format!(
                                "Add a type after the parameter name, for example `{}: I32`.",
                                param.name
                            )),
                    )),
                    (Some(ty), None) => {
                        self.validate_type_in_scope(ty, &function.type_params)?;
                        Ok(normalize_type_params(ty, &function.type_params))
                    }
                })
                .collect::<Result<Vec<_>>>()?;
            let ret_type = match &function.return_annotation {
                Some(ty) => {
                    let span = function
                        .return_annotation_span
                        .clone()
                        .unwrap_or_else(|| self.placeholder_span());
                    self.validate_type_in_scope_at(ty, &function.type_params, &span)?;
                    normalize_type_params(ty, &function.type_params)
                }
                None => {
                    return Err(Error::diagnostic(
                        Diagnostic::new("Missing return type")
                            .label(
                                function.name_span.clone(),
                                format!(
                                    "function `{}` must have a return type annotation.",
                                    function.name
                                ),
                            )
                            .help("Add `-> Type` before the function body."),
                    ));
                }
            };
            let params = param_types
                .iter()
                .cloned()
                .map(|ty| self.known(ty))
                .collect();
            let ret = self.known(ret_type.clone());
            self.functions.insert(
                function.name.clone(),
                FunctionType {
                    type_params: function.type_params.clone(),
                    param_types,
                    ret_type,
                    params,
                    ret,
                    effectful,
                    declared_capabilities,
                },
            );
        }
        Ok(())
    }

    fn check_main(&self) -> Result<()> {
        let functions = self.program.functions();
        let entry_count = functions
            .iter()
            .filter(|function| function.name == "main" || function.name == "main!")
            .count();
        if entry_count != 1 {
            return Err(Error::diagnostic(
                Diagnostic::new("Missing entrypoint")
                    .label(
                        self.placeholder_span(),
                        "I could not find exactly one top-level `main` or `main!` function.",
                    )
                    .help("Define one executable entrypoint named `main` or `main!`."),
            ));
        }
        let main = functions
            .iter()
            .find(|function| function.name == "main" || function.name == "main!")
            .expect("entry point was counted above");
        if !main.params.is_empty() {
            return Err(Error::diagnostic(
                Diagnostic::new("Invalid entrypoint")
                    .label(
                        main.name_span.clone(),
                        "`main` and `main!` must take zero parameters.",
                    )
                    .help("Remove the parameters from the entrypoint."),
            ));
        }
        if !main.type_params.is_empty() {
            return Err(Error::diagnostic(
                Diagnostic::new("Invalid entrypoint")
                    .label(
                        main.name_span.clone(),
                        "`main` and `main!` must not be generic.",
                    )
                    .help("Remove the `<...>` type parameter list from the entrypoint."),
            ));
        }
        Ok(())
    }

    fn check_runtime_boundary(
        &self,
        function_capabilities: &HashMap<String, BTreeSet<Capability>>,
    ) -> Result<()> {
        let functions = self.program.functions();
        let entry = functions
            .iter()
            .find(|function| function.name == "main" || function.name == "main!")
            .expect("entry point was checked above");
        let required = function_capabilities
            .get(&entry.name)
            .cloned()
            .unwrap_or_default();
        let provided = &self.platform.provided_capabilities;
        if !required.is_subset(provided) {
            let missing = required
                .difference(provided)
                .map(|capability| format!("{capability:?}"))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(Error::diagnostic(
                Diagnostic::new("Missing platform capability")
                    .label(
                        entry.name_span.clone(),
                        format!(
                            "platform `{}` does not provide required capability: {missing}.",
                            self.platform
                        ),
                    )
                    .help("Use a backend that provides this capability, or remove the operation that requires it."),
            ));
        }
        Ok(())
    }

    fn return_mismatch_span(
        &mut self,
        function: &Function,
        expected_id: usize,
        actual_id: usize,
        fallback: Option<&Span>,
    ) -> Span {
        let fallback = fallback
            .cloned()
            .or_else(|| function.return_annotation_span.clone())
            .unwrap_or_else(|| self.placeholder_span());
        let Some(expected) = self.resolve_optional(expected_id) else {
            return fallback;
        };
        let Some(actual) = self.resolve_optional(actual_id) else {
            return fallback;
        };
        let mut mismatches = Vec::new();
        collect_generic_param_mismatches(&expected, &actual, &mut mismatches);
        if mismatches.is_empty() {
            return fallback;
        }

        for (_expected, actual) in mismatches {
            if let Some(span) = function.params.iter().find_map(|param| {
                let ty = param.ty.as_ref()?;
                if function_type_return_contains_generic_param(ty, &actual) {
                    param.ty_span.clone()
                } else {
                    None
                }
            }) {
                return span;
            }
        }

        function.return_annotation_span.clone().unwrap_or(fallback)
    }

    fn check_block(
        &mut self,
        block: &Block,
        outer_scope: &mut HashMap<String, usize>,
    ) -> Result<ExprInfo> {
        let mut scope = outer_scope.clone();
        let mut last_expr = None;
        let mut effectful = false;
        let mut capabilities = BTreeSet::new();
        for item in &block.items {
            match item {
                BlockItem::Binding {
                    name,
                    ty,
                    ty_span,
                    expr,
                    span,
                } => {
                    if scope.contains_key(name) {
                        return Err(Error::diagnostic(
                            Diagnostic::new("Duplicate binding")
                                .label(
                                    span.clone(),
                                    format!("duplicate binding `{name}` in the same block."),
                                )
                                .help("Use a different local name, or remove the earlier binding."),
                        ));
                    }
                    let info = self.check_expr(expr, &mut scope)?;
                    let Some(annotation) = ty else {
                        return Err(Error::diagnostic(
                            Diagnostic::new("Missing type annotation")
                                .label(
                                    span.clone(),
                                    format!("binding `{name}` must have a type annotation."),
                                )
                                .help(format!(
                                    "Add a type annotation, for example `{name}: I32 = ...`."
                                )),
                        ));
                    };
                    if let Some(ty_span) = ty_span {
                        self.validate_type_in_scope_at(annotation, &[], ty_span)?;
                    } else {
                        self.validate_type(annotation)?;
                    }
                    let annotated_ty = self.known(annotation.clone());
                    self.unify_at(
                        info.ty,
                        annotated_ty,
                        expr.span().clone(),
                        format!("This value must match the annotation for `{name}`."),
                    )?;
                    effectful |= info.effectful;
                    capabilities.extend(info.capabilities);
                    scope.insert(name.clone(), info.ty);
                    last_expr = None;
                }
                BlockItem::Expr(expr) => {
                    let info = self.check_expr(expr, &mut scope)?;
                    effectful |= info.effectful;
                    capabilities.extend(info.capabilities.clone());
                    last_expr = Some(info);
                }
            }
        }
        Ok(ExprInfo {
            ty: last_expr
                .as_ref()
                .map(|info| info.ty)
                .unwrap_or_else(|| self.known(Type::Prim(PrimType::Unit))),
            effectful,
            capabilities,
            span: last_expr.and_then(|info| info.span),
        })
    }

    fn check_expr(&mut self, expr: &Expr, scope: &mut HashMap<String, usize>) -> Result<ExprInfo> {
        match expr {
            Expr::Int(_, _) => {
                let ty = self.known(Type::Prim(PrimType::I32));
                Ok(self.info_at(ty, expr.span().clone()))
            }
            Expr::Bool(_, _) => {
                let ty = self.known(Type::Prim(PrimType::Bool));
                Ok(self.info_at(ty, expr.span().clone()))
            }
            Expr::String(_, _) => {
                let ty = self.known(Type::Prim(PrimType::String));
                Ok(self.info_at(ty, expr.span().clone()))
            }
            Expr::Unit(_) => {
                let ty = self.known(Type::Prim(PrimType::Unit));
                Ok(self.info_at(ty, expr.span().clone()))
            }
            Expr::Var(name, span) => {
                if let Some(ty) = scope.get(name).copied() {
                    return Ok(self.info_at(ty, expr.span().clone()));
                }
                if let Some(variant) = self.variants.get(name).cloned() {
                    if variant.payload.is_some() {
                        return Err(Error::diagnostic(
                            Diagnostic::new("Missing enum payload")
                                .label(
                                    span.clone(),
                                    format!("enum variant `{name}` requires a payload."),
                                )
                                .help("Call this variant with its payload value."),
                        ));
                    }
                    let ty = self.known(enum_result_type(&variant, Vec::new()));
                    return Ok(self.info_at(ty, expr.span().clone()));
                }
                if let Some(function_ty) = self.function_value_type(name) {
                    let ty = self.known(function_ty);
                    return Ok(self.info_at(ty, expr.span().clone()));
                }
                Err(Error::diagnostic(
                    Diagnostic::new("Unknown name")
                        .label(
                            span.clone(),
                            format!("I cannot find `{name}` in this scope."),
                        )
                        .help("Check the spelling, or define this name before using it."),
                ))
            }
            Expr::Call {
                name,
                type_args,
                args,
                ..
            } => {
                if let Some(variant) = self.variants.get(name).cloned() {
                    if !type_args.is_empty() {
                        return Err(Error::diagnostic(
                            Diagnostic::new("Unexpected type arguments")
                                .label(
                                    expr.span().clone(),
                                    format!("enum variant `{name}` does not take type arguments."),
                                )
                                .help(
                                    "Remove the `<...>` type argument list from this constructor.",
                                ),
                        ));
                    }
                    return self.check_variant_constructor(name, &variant, args, scope);
                }

                if let Some(callee_ty) = scope.get(name).copied() {
                    if !type_args.is_empty() {
                        return Err(Error::diagnostic(
                            Diagnostic::new("Unexpected type arguments")
                                .label(
                                    expr.span().clone(),
                                    format!(
                                        "function value `{name}` does not take type arguments."
                                    ),
                                )
                                .help("Remove the `<...>` type argument list from this call."),
                        ));
                    }
                    return self.check_function_value_call(name, callee_ty, args, scope);
                }

                let signature = self.functions.get(name).cloned().ok_or_else(|| {
                    Error::diagnostic(
                        Diagnostic::new("Unknown function")
                            .label(
                                expr.span().clone(),
                                format!("I cannot find a function named `{name}`."),
                            )
                            .help("Check the spelling, define the function, or import it before calling it."),
                    )
                })?;
                if !signature.type_params.is_empty() {
                    return self.check_generic_function_call(
                        name,
                        &signature,
                        type_args,
                        args,
                        expr.span().clone(),
                        scope,
                    );
                }
                if !type_args.is_empty() {
                    return Err(Error::diagnostic(
                        Diagnostic::new("Unexpected type arguments")
                            .label(
                                expr.span().clone(),
                                format!(
                                    "non-generic function `{name}` does not take type arguments."
                                ),
                            )
                            .help("Remove the `<...>` type argument list from this call."),
                    ));
                }
                if args.len() != signature.params.len() {
                    return Err(Error::diagnostic(
                        Diagnostic::new("Wrong number of arguments")
                            .label(
                                expr.span().clone(),
                                format!(
                                    "function `{name}` expects {} argument(s), got {}.",
                                    signature.params.len(),
                                    args.len()
                                ),
                            )
                            .help(
                                "Change the argument list so it matches the function definition.",
                            ),
                    ));
                }

                let mut effectful = signature.effectful;
                let mut capabilities = signature.declared_capabilities.clone().unwrap_or_default();
                for (arg, param_ty) in args.iter().zip(signature.params.iter()) {
                    let arg = self.check_expr(arg, scope)?;
                    self.unify_at(
                        arg.ty,
                        *param_ty,
                        arg.span.clone().unwrap_or_else(|| self.placeholder_span()),
                        format!("This argument does not match parameter type for `{name}`."),
                    )?;
                    effectful |= arg.effectful;
                    capabilities.extend(arg.capabilities);
                }
                Ok(ExprInfo {
                    ty: signature.ret,
                    effectful,
                    capabilities,
                    span: Some(expr.span().clone()),
                })
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                ..
            } => self.check_method_call(receiver, name, args, scope),
            Expr::FieldAccess {
                receiver, field, ..
            } => {
                let receiver = self.check_expr(receiver, scope)?;
                let receiver_ty = self.resolve_known(receiver.ty, "field receiver type")?;
                let Type::Named(type_name) = receiver_ty else {
                    return Err(Error::diagnostic(
                        Diagnostic::new("Type has no fields")
                            .label(
                                expr.span().clone(),
                                format!(
                                    "type {} does not have field `{field}`.",
                                    format_type(&receiver_ty)
                                ),
                            )
                            .help("Only struct values have fields."),
                    ));
                };
                let decl = self.structs.get(&type_name).ok_or_else(|| {
                    Error::diagnostic(
                        Diagnostic::new("Type has no fields")
                            .label(
                                receiver
                                    .span
                                    .clone()
                                    .unwrap_or_else(|| self.placeholder_span()),
                                format!("type `{type_name}` has no fields."),
                            )
                            .help("Only struct values have fields."),
                    )
                })?;
                if decl.field.name != *field {
                    return Err(Error::diagnostic(
                        Diagnostic::new("Unknown field")
                            .label(
                                expr.span().clone(),
                                format!("struct `{type_name}` does not have field `{field}`."),
                            )
                            .help(format!("The available field is `{}`.", decl.field.name)),
                    ));
                }
                let ty = self.known(decl.field.ty.clone());
                Ok(ExprInfo {
                    ty,
                    effectful: receiver.effectful,
                    capabilities: receiver.capabilities,
                    span: Some(expr.span().clone()),
                })
            }
            Expr::StructLiteral {
                name, field, value, ..
            } => {
                let decl = self.structs.get(name).ok_or_else(|| {
                    Error::diagnostic(
                        Diagnostic::new("Unknown struct")
                            .label(
                                expr.span().clone(),
                                format!("I cannot find struct `{name}`."),
                            )
                            .help("Check the spelling, or define the struct before using it."),
                    )
                })?;
                if decl.field.name != *field {
                    return Err(Error::diagnostic(
                        Diagnostic::new("Unknown field")
                            .label(
                                expr.span().clone(),
                                format!("struct `{name}` does not have field `{field}`."),
                            )
                            .help(format!("The available field is `{}`.", decl.field.name)),
                    ));
                }
                let expected_ty = decl.field.ty.clone();
                let value = self.check_expr(value, scope)?;
                let expected_ty = self.known(expected_ty);
                self.unify_at(
                    value.ty,
                    expected_ty,
                    value
                        .span
                        .clone()
                        .unwrap_or_else(|| self.placeholder_span()),
                    format!("This value does not match field `{field}`."),
                )?;
                Ok(ExprInfo {
                    ty: self.known(Type::Named(name.clone())),
                    effectful: value.effectful,
                    capabilities: value.capabilities,
                    span: Some(expr.span().clone()),
                })
            }
            Expr::Binary {
                op, left, right, ..
            } => self.check_binary(*op, left, right, scope),
            Expr::Match {
                scrutinee, arms, ..
            } => self.check_match(scrutinee, arms, scope),
            Expr::Lambda { params, body, span } => self.check_lambda(params, body, span, scope),
            Expr::Block(block, _) => self.check_block(block, scope),
        }
    }

    fn check_lambda(
        &mut self,
        params: &[crate::ast::FunctionParam],
        body: &Expr,
        span: &Span,
        scope: &mut HashMap<String, usize>,
    ) -> Result<ExprInfo> {
        let type_param_scope = self.rigid_type_params.iter().cloned().collect::<Vec<_>>();
        let mut local_scope = scope.clone();
        let mut seen_params = BTreeSet::new();
        let mut param_types = Vec::new();

        for param in params {
            if !seen_params.insert(param.name.clone()) {
                return Err(Error::diagnostic(
                    Diagnostic::new("Duplicate parameter")
                        .label(
                            param.name_span.clone(),
                            format!(
                                "parameter `{}` is already defined in this anonymous function.",
                                param.name
                            ),
                        )
                        .help("Rename one of the parameters so each name is unique."),
                ));
            }

            let Some(annotation) = &param.ty else {
                return Err(Error::diagnostic(
                    Diagnostic::new("Missing type annotation")
                        .label(
                            param.name_span.clone(),
                            format!(
                                "parameter `{}` in this anonymous function must have a type annotation.",
                                param.name
                            ),
                        )
                        .help(format!(
                            "Add a type after the parameter name, for example `{}: I32`.",
                            param.name
                        )),
                ));
            };
            if let Some(ty_span) = &param.ty_span {
                self.validate_type_in_scope_at(annotation, &type_param_scope, ty_span)?;
            } else {
                self.validate_type_in_scope(annotation, &type_param_scope)?;
            }
            let normalized = normalize_type_params(annotation, &type_param_scope);
            let id = self.known(normalized.clone());
            local_scope.insert(param.name.clone(), id);
            param_types.push(normalized);
        }

        let body = self.check_expr(body, &mut local_scope)?;
        let ret = self.resolve_known(body.ty, "anonymous function return type")?;
        let ty = self.known(Type::Function(AstFunctionType {
            params: param_types,
            ret: Box::new(ret),
            effectful: body.effectful,
        }));
        Ok(ExprInfo {
            ty,
            effectful: false,
            capabilities: BTreeSet::new(),
            span: Some(span.clone()),
        })
    }

    fn check_generic_function_call(
        &mut self,
        name: &str,
        signature: &FunctionType,
        explicit_type_args: &[Type],
        args: &[Expr],
        call_span: Span,
        scope: &mut HashMap<String, usize>,
    ) -> Result<ExprInfo> {
        let mut substitutions = HashMap::new();
        if !explicit_type_args.is_empty() {
            if explicit_type_args.len() != signature.type_params.len() {
                return Err(Error::diagnostic(
                    Diagnostic::new("Wrong number of type arguments")
                        .label(
                            call_span.clone(),
                            format!(
                                "function `{name}` expects {} type argument(s), got {}.",
                                signature.type_params.len(),
                                explicit_type_args.len()
                            ),
                        )
                        .help("Change the `<...>` type argument list so it matches the function definition."),
                ));
            }
            let type_param_scope = self.rigid_type_params.iter().cloned().collect::<Vec<_>>();
            for (param, arg) in signature.type_params.iter().zip(explicit_type_args.iter()) {
                self.validate_type_in_scope_at(arg, &type_param_scope, &call_span)?;
                substitutions.insert(param.clone(), normalize_type_params(arg, &type_param_scope));
            }
        }

        if args.len() != signature.param_types.len() {
            return Err(Error::diagnostic(
                Diagnostic::new("Wrong number of arguments")
                    .label(
                        call_span,
                        format!(
                            "function `{name}` expects {} argument(s), got {}.",
                            signature.param_types.len(),
                            args.len()
                        ),
                    )
                    .help("Change the argument list so it matches the function definition."),
            ));
        }

        let mut checked_args = Vec::new();
        let mut effectful = signature.effectful;
        let mut capabilities = signature.declared_capabilities.clone().unwrap_or_default();
        for (arg, param_ty) in args.iter().zip(signature.param_types.iter()) {
            let arg = self.check_expr(arg, scope)?;
            let arg_ty = self.resolve_known(arg.ty, "generic function argument type")?;
            infer_type_arguments(param_ty, &arg_ty, &mut substitutions).map_err(|err| {
                Error::diagnostic(
                    Diagnostic::new("Conflicting type argument")
                        .label(call_span.clone(), err.to_string())
                        .help("Make the explicit type arguments and value arguments agree."),
                )
            })?;
            effectful |= arg.effectful;
            capabilities.extend(arg.capabilities.clone());
            checked_args.push((arg, param_ty));
        }

        for (arg, param_ty) in checked_args {
            let expected_ty = substitute_type(param_ty, &substitutions);
            let expected = self.known(expected_ty);
            self.unify_at(
                arg.ty,
                expected,
                arg.span.clone().unwrap_or_else(|| self.placeholder_span()),
                format!("This argument does not match the generic parameter for `{name}`."),
            )?;
        }

        Ok(ExprInfo {
            ty: self.known(substitute_type(&signature.ret_type, &substitutions)),
            effectful,
            capabilities,
            span: None,
        })
    }

    fn check_variant_constructor(
        &mut self,
        name: &str,
        variant: &VariantInfo,
        args: &[Expr],
        scope: &mut HashMap<String, usize>,
    ) -> Result<ExprInfo> {
        let Some(payload_ty) = &variant.payload else {
            return Err(Error::diagnostic(
                Diagnostic::new("Unexpected enum payload")
                    .label(
                        args.first()
                            .map(|arg| arg.span().clone())
                            .unwrap_or_else(|| self.placeholder_span()),
                        format!("enum variant `{name}` does not take a payload."),
                    )
                    .help("Remove the payload argument from this variant constructor."),
            ));
        };
        if args.len() != 1 {
            return Err(Error::diagnostic(
                Diagnostic::new("Wrong number of enum payloads")
                    .label(
                        args.first()
                            .map(|arg| arg.span().clone())
                            .unwrap_or_else(|| self.placeholder_span()),
                        format!(
                            "enum variant `{name}` expects 1 payload argument, got {}.",
                            args.len()
                        ),
                    )
                    .help("Pass exactly one payload value to this variant."),
            ));
        }
        let mut substitutions = HashMap::new();
        let arg = self.check_expr(&args[0], scope)?;
        let arg_ty = self.resolve_known(arg.ty, "enum variant payload type")?;
        infer_type_arguments(payload_ty, &arg_ty, &mut substitutions).map_err(|err| {
            Error::diagnostic(
                Diagnostic::new("Conflicting enum payload type")
                    .label(args[0].span().clone(), err.to_string())
                    .help("Make the enum payload match the expected variant type."),
            )
        })?;
        let result_args = variant
            .enum_type_params
            .iter()
            .map(|param| {
                substitutions
                    .get(param)
                    .cloned()
                    .unwrap_or_else(|| Type::GenericParam(param.clone()))
            })
            .collect::<Vec<_>>();
        let expected_ty = substitute_type(payload_ty, &substitutions);
        let expected = self.known(expected_ty);
        self.unify_at(
            arg.ty,
            expected,
            args[0].span().clone(),
            format!("This payload does not match enum variant `{name}`."),
        )?;
        Ok(ExprInfo {
            ty: self.known(enum_result_type(variant, result_args)),
            effectful: arg.effectful,
            capabilities: arg.capabilities,
            span: Some(args[0].span().clone()),
        })
    }

    fn check_function_value_call(
        &mut self,
        name: &str,
        callee_ty: usize,
        args: &[Expr],
        scope: &mut HashMap<String, usize>,
    ) -> Result<ExprInfo> {
        let callee_ty = self.resolve_known(callee_ty, "function value callee type")?;
        let Type::Function(function_ty) = callee_ty else {
            return Err(Error::diagnostic(
                Diagnostic::new("Not callable")
                    .label(
                        self.placeholder_span(),
                        format!("`{name}` is not callable."),
                    )
                    .help("Only functions and function values can be called."),
            ));
        };
        if args.len() != function_ty.params.len() {
            return Err(Error::diagnostic(
                Diagnostic::new("Wrong number of arguments")
                    .label(
                        args.first()
                            .map(|arg| arg.span().clone())
                            .unwrap_or_else(|| self.placeholder_span()),
                        format!(
                            "function value `{name}` expects {} argument(s), got {}.",
                            function_ty.params.len(),
                            args.len()
                        ),
                    )
                    .help("Change the argument list so it matches the function value type."),
            ));
        }

        let mut effectful = function_ty.effectful;
        let mut capabilities = BTreeSet::new();
        for (arg, param_ty) in args.iter().zip(function_ty.params.iter()) {
            let arg = self.check_expr(arg, scope)?;
            let param_ty = self.known(param_ty.clone());
            self.unify_at(
                arg.ty,
                param_ty,
                arg.span.clone().unwrap_or_else(|| self.placeholder_span()),
                format!("This argument does not match function value `{name}`."),
            )?;
            effectful |= arg.effectful;
            capabilities.extend(arg.capabilities);
        }

        Ok(ExprInfo {
            ty: self.known(*function_ty.ret),
            effectful,
            capabilities,
            span: None,
        })
    }

    fn check_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[MatchArm],
        scope: &mut HashMap<String, usize>,
    ) -> Result<ExprInfo> {
        if arms.is_empty() {
            return Err(Error::diagnostic(
                Diagnostic::new("Empty match")
                    .label(
                        scrutinee.span().clone(),
                        "match expression must have at least one arm.",
                    )
                    .help("Add at least one pattern arm inside the match body."),
            ));
        }

        let scrutinee = self.check_expr(scrutinee, scope)?;
        let scrutinee_ty = self.resolve_known(scrutinee.ty, "match scrutinee type")?;
        let mut effectful = scrutinee.effectful;
        let mut capabilities = scrutinee.capabilities;
        let mut result_ty = None;

        for arm in arms {
            let mut arm_scope = scope.clone();
            let scrutinee_span = scrutinee
                .span
                .clone()
                .unwrap_or_else(|| self.placeholder_span());
            self.check_pattern(&arm.pattern, &scrutinee_ty, &mut arm_scope, &scrutinee_span)?;
            let arm = self.check_expr(&arm.expr, &mut arm_scope)?;
            effectful |= arm.effectful;
            capabilities.extend(arm.capabilities);
            if let Some(existing) = result_ty {
                self.unify_at(
                    existing,
                    arm.ty,
                    arm.span.clone().unwrap_or_else(|| self.placeholder_span()),
                    "This match arm returns a different type from the previous arms.",
                )?;
            } else {
                result_ty = Some(arm.ty);
            }
        }

        if !self.match_is_exhaustive(&scrutinee_ty, arms) {
            return Err(Error::diagnostic(
                Diagnostic::new("Match is not exhaustive")
                    .label(
                        scrutinee
                            .span
                            .clone()
                            .unwrap_or_else(|| self.placeholder_span()),
                        "This match does not cover every possible value.",
                    )
                    .help("Add the missing patterns, or add a `_` wildcard arm."),
            ));
        }

        Ok(ExprInfo {
            ty: result_ty.expect("non-empty arms checked above"),
            effectful,
            capabilities,
            span: scrutinee.span,
        })
    }

    fn check_pattern(
        &mut self,
        pattern: &Pattern,
        expected: &Type,
        scope: &mut HashMap<String, usize>,
        span: &Span,
    ) -> Result<()> {
        match pattern {
            Pattern::Int(_) => {
                self.expect_pattern_type_at(expected, Type::Prim(PrimType::I32), span)
            }
            Pattern::Bool(_) => {
                self.expect_pattern_type_at(expected, Type::Prim(PrimType::Bool), span)
            }
            Pattern::Unit => {
                self.expect_pattern_type_at(expected, Type::Prim(PrimType::Unit), span)
            }
            Pattern::Wildcard => Ok(()),
            Pattern::Var(name) => {
                if scope.contains_key(name) {
                    return Err(Error::diagnostic(
                        Diagnostic::new("Duplicate pattern binding")
                            .label(
                                span.clone(),
                                format!("pattern binding `{name}` shadows an existing binding."),
                            )
                            .help(
                                "Use a different name in the pattern, or remove the outer binding.",
                            ),
                    ));
                }
                let ty = self.known(expected.clone());
                scope.insert(name.clone(), ty);
                Ok(())
            }
            Pattern::Variant { name, payload } => {
                let variant = self.variants.get(name).cloned().ok_or_else(|| {
                    Error::diagnostic(
                        Diagnostic::new("Unknown enum variant")
                            .label(span.clone(), format!("unknown enum variant `{name}`."))
                            .help("Check the variant name, or define it before matching on it."),
                    )
                })?;
                let substitutions = self.pattern_type_arguments(expected, &variant, span)?;
                self.expect_pattern_type_at(
                    expected,
                    enum_result_type(&variant, substitutions.clone()),
                    span,
                )?;
                match (&variant.payload, payload) {
                    (Some(payload_ty), Some(payload_pattern)) => {
                        let payload_ty = substitute_type_for_params(
                            payload_ty,
                            &variant.enum_type_params,
                            &substitutions,
                        );
                        self.check_pattern(payload_pattern, &payload_ty, scope, span)
                    }
                    (Some(_), None) => Err(Error::diagnostic(
                        Diagnostic::new("Missing enum payload pattern")
                            .label(
                                span.clone(),
                                format!("enum variant `{name}` pattern requires a payload."),
                            )
                            .help("Add a payload pattern, for example `Variant(value)`."),
                    )),
                    (None, Some(_)) => Err(Error::diagnostic(
                        Diagnostic::new("Unexpected enum payload pattern")
                            .label(
                                span.clone(),
                                format!("enum variant `{name}` pattern does not take a payload."),
                            )
                            .help("Remove the payload pattern from this variant."),
                    )),
                    (None, None) => Ok(()),
                }
            }
        }
    }

    fn expect_pattern_type_at(&self, expected: &Type, actual: Type, span: &Span) -> Result<()> {
        if *expected == actual {
            Ok(())
        } else {
            Err(Error::diagnostic(
                Diagnostic::new("Pattern type mismatch")
                    .message("This is a type mismatch.")
                    .label(
                        span.clone(),
                        format!(
                            "This pattern expects `{}`, but the matched value has type `{}`.",
                            format_type(&actual),
                            format_type(expected)
                        ),
                    )
                    .help("Change the pattern so it matches the value being inspected."),
            ))
        }
    }

    fn check_method_call(
        &mut self,
        receiver: &Expr,
        name: &str,
        args: &[Expr],
        scope: &mut HashMap<String, usize>,
    ) -> Result<ExprInfo> {
        let receiver = self.check_expr(receiver, scope)?;
        let mut effectful = receiver.effectful;
        let mut capabilities = receiver.capabilities;

        let (receiver_constraint, expected_args, ret) = match name {
            "add" | "sub" | "mul" => (Some(PrimType::I32), vec![PrimType::I32], PrimType::I32),
            "lt" => (Some(PrimType::I32), vec![PrimType::I32], PrimType::Bool),
            "eq" => {
                let receiver_ty = self.resolve_optional(receiver.ty);
                match receiver_ty {
                    Some(Type::Prim(PrimType::I32)) => {
                        (Some(PrimType::I32), vec![PrimType::I32], PrimType::Bool)
                    }
                    Some(Type::Prim(PrimType::Bool)) => {
                        (Some(PrimType::Bool), vec![PrimType::Bool], PrimType::Bool)
                    }
                    Some(Type::Prim(PrimType::Unit)) => {
                        return Err(Error::diagnostic(
                            Diagnostic::new("Method not available")
                                .label(
                                    receiver
                                        .span
                                        .clone()
                                        .unwrap_or_else(|| self.placeholder_span()),
                                    "type Unit does not implement method `eq`.",
                                )
                                .help("Compare values with a type that supports equality."),
                        ));
                    }
                    Some(other) => {
                        return Err(Error::diagnostic(
                            Diagnostic::new("Method not available")
                                .label(
                                    receiver
                                        .span
                                        .clone()
                                        .unwrap_or_else(|| self.placeholder_span()),
                                    format!(
                                        "type {} does not implement method `eq`.",
                                        format_type(&other)
                                    ),
                                )
                                .help(
                                    "Only compatible primitive values can be compared with `eq`.",
                                ),
                        ));
                    }
                    None => (None, Vec::new(), PrimType::Bool),
                }
            }
            _ => {
                let receiver_ty = self
                    .resolve_optional(receiver.ty)
                    .map(|ty| format_type(&ty))
                    .unwrap_or_else(|| "unknown type".to_string());
                return Err(Error::diagnostic(
                    Diagnostic::new("Method not available")
                        .label(
                            receiver
                                .span
                                .clone()
                                .unwrap_or_else(|| self.placeholder_span()),
                            format!("{receiver_ty} does not implement method `{name}`."),
                        )
                        .help("Use a method supported by this value's type."),
                ));
            }
        };

        if name == "eq" && receiver_constraint.is_none() {
            if args.len() != 1 {
                return Err(Error::diagnostic(
                    Diagnostic::new("Wrong number of method arguments")
                        .label(
                            args.first()
                                .map(|arg| arg.span().clone())
                                .or_else(|| receiver.span.clone())
                                .unwrap_or_else(|| self.placeholder_span()),
                            format!("method `{name}` expects 1 argument(s), got {}.", args.len()),
                        )
                        .help("Change the argument list so it matches the method."),
                ));
            }
            let arg = self.check_expr(&args[0], scope)?;
            self.unify_at(
                receiver.ty,
                arg.ty,
                args[0].span().clone(),
                "Both sides of `==` must have the same type.",
            )?;
            effectful |= arg.effectful;
            capabilities.extend(arg.capabilities);
            return Ok(ExprInfo {
                ty: self.known(Type::Prim(ret)),
                effectful,
                capabilities,
                span: receiver.span.clone(),
            });
        }

        if let Some(receiver_constraint) = receiver_constraint {
            let receiver_constraint = self.known(Type::Prim(receiver_constraint));
            self.unify_at(
                receiver.ty,
                receiver_constraint,
                receiver
                    .span
                    .clone()
                    .unwrap_or_else(|| self.placeholder_span()),
                format!("The receiver of `{name}` must have this type."),
            )?;
        }

        if args.len() != expected_args.len() {
            return Err(Error::diagnostic(
                Diagnostic::new("Wrong number of method arguments")
                    .label(
                        args.first()
                            .map(|arg| arg.span().clone())
                            .or_else(|| receiver.span.clone())
                            .unwrap_or_else(|| self.placeholder_span()),
                        format!(
                            "method `{name}` expects {} argument(s), got {}.",
                            expected_args.len(),
                            args.len()
                        ),
                    )
                    .help("Change the argument list so it matches the method."),
            ));
        }

        for (arg, expected_ty) in args.iter().zip(expected_args.iter()) {
            let arg = self.check_expr(arg, scope)?;
            let expected_ty = self.known(Type::Prim(*expected_ty));
            self.unify_at(
                arg.ty,
                expected_ty,
                arg.span.clone().unwrap_or_else(|| self.placeholder_span()),
                format!("This argument does not match method `{name}`."),
            )?;
            effectful |= arg.effectful;
            capabilities.extend(arg.capabilities);
        }

        Ok(ExprInfo {
            ty: self.known(Type::Prim(ret)),
            effectful,
            capabilities,
            span: receiver.span,
        })
    }

    fn check_binary(
        &mut self,
        op: BinaryOp,
        left: &Expr,
        right: &Expr,
        scope: &mut HashMap<String, usize>,
    ) -> Result<ExprInfo> {
        let method = match op {
            BinaryOp::Add => "add",
            BinaryOp::Sub => "sub",
            BinaryOp::Mul => "mul",
            BinaryOp::Eq => "eq",
            BinaryOp::Lt => "lt",
        };
        self.check_method_call(left, method, std::slice::from_ref(right), scope)
    }

    fn match_is_exhaustive(&self, scrutinee: &Type, arms: &[MatchArm]) -> bool {
        if arms
            .iter()
            .any(|arm| matches!(arm.pattern, Pattern::Wildcard | Pattern::Var(_)))
        {
            return true;
        }

        match scrutinee {
            Type::Prim(PrimType::Bool) => {
                let has_true = arms
                    .iter()
                    .any(|arm| matches!(arm.pattern, Pattern::Bool(true)));
                let has_false = arms
                    .iter()
                    .any(|arm| matches!(arm.pattern, Pattern::Bool(false)));
                has_true && has_false
            }
            Type::Prim(PrimType::Unit) => {
                arms.iter().any(|arm| matches!(arm.pattern, Pattern::Unit))
            }
            Type::Prim(PrimType::I32) | Type::Prim(PrimType::String) => false,
            Type::Function(_) | Type::GenericParam(_) => false,
            Type::Named(name) => self.enum_variant_names(name).is_some_and(|variants| {
                variants.iter().all(|variant_name| {
                    arms.iter().any(|arm| match &arm.pattern {
                        Pattern::Variant { name, .. } => name == variant_name,
                        _ => false,
                    })
                })
            }),
            Type::Apply { name, .. } => self.enum_variant_names(name).is_some_and(|variants| {
                variants.iter().all(|variant_name| {
                    arms.iter().any(|arm| match &arm.pattern {
                        Pattern::Variant { name, .. } => name == variant_name,
                        _ => false,
                    })
                })
            }),
        }
    }

    fn enum_variant_names(&self, name: &str) -> Option<Vec<String>> {
        if name == "Result" {
            return Some(vec!["Ok".to_string(), "Err".to_string()]);
        }
        if name == "PlatformError" {
            return Some(
                [
                    "Unsupported",
                    "Unavailable",
                    "Interrupted",
                    "InvalidUtf8",
                    "Unknown",
                ]
                .into_iter()
                .map(str::to_string)
                .collect(),
            );
        }
        self.enums.get(name).map(|decl| {
            decl.variants
                .iter()
                .map(|variant| variant.name.clone())
                .collect()
        })
    }

    fn validate_type(&self, ty: &Type) -> Result<()> {
        self.validate_type_in_scope(ty, &[])
    }

    fn validate_type_in_scope_at(
        &self,
        ty: &Type,
        type_params: &[String],
        span: &Span,
    ) -> Result<()> {
        match self.validate_type_in_scope(ty, type_params) {
            Ok(()) => Ok(()),
            Err(_) => match ty {
                Type::Named(name) => Err(Error::diagnostic(
                    Diagnostic::new("Unknown type")
                        .label(
                            span.clone(),
                            format!("I cannot find a type named `{name}`."),
                        )
                        .help("Check the spelling, or define this type before using it."),
                )),
                Type::GenericParam(name) => Err(Error::diagnostic(
                    Diagnostic::new("Unknown type parameter")
                        .label(
                            span.clone(),
                            format!("`{name}` is not in scope as a type parameter."),
                        )
                        .help("Add it to the surrounding `<...>` type parameter list."),
                )),
                Type::Apply { name, args } => {
                    if name != "Result"
                        && !self.structs.contains_key(name)
                        && !self.enums.contains_key(name)
                    {
                        return Err(Error::diagnostic(
                            Diagnostic::new("Unknown generic type")
                                .label(
                                    span.clone(),
                                    format!("I cannot find a generic type named `{name}`."),
                                )
                                .help("Check the type name, or define it before using it."),
                        ));
                    }
                    let expected = if name == "Result" {
                        2
                    } else if let Some(decl) = self.structs.get(name) {
                        decl.type_params.len()
                    } else {
                        self.enums
                            .get(name)
                            .map(|decl| decl.type_params.len())
                            .unwrap_or_default()
                    };
                    if args.len() != expected {
                        return Err(Error::diagnostic(
                            Diagnostic::new("Wrong number of type arguments")
                                .label(
                                    span.clone(),
                                    format!(
                                        "`{name}` expects {expected} type argument(s), but got {}.",
                                        args.len()
                                    ),
                                )
                                .help("Change the type argument list so it matches the type definition."),
                        ));
                    }
                    Err(Error::diagnostic(
                        Diagnostic::new("Invalid type")
                            .label(span.clone(), "One of the nested types is not valid.")
                            .help("Check the type arguments inside this type."),
                    ))
                }
                Type::Function(_) => Err(Error::diagnostic(
                    Diagnostic::new("Invalid function type")
                        .label(
                            span.clone(),
                            "One of the types inside this function type is invalid.",
                        )
                        .help("Check the parameter and return types."),
                )),
                Type::Prim(_) => unreachable!("primitive types are always valid"),
            },
        }
    }

    fn validate_type_in_scope(&self, ty: &Type, type_params: &[String]) -> Result<()> {
        match ty {
            Type::Prim(_) => Ok(()),
            Type::Named(name) => {
                if type_params.contains(name)
                    || name == "PlatformError"
                    || self
                        .structs
                        .get(name)
                        .is_some_and(|decl| decl.type_params.is_empty())
                    || self
                        .enums
                        .get(name)
                        .is_some_and(|decl| decl.type_params.is_empty())
                {
                    Ok(())
                } else {
                    Err(Error::new(format!("unknown type `{name}`")))
                }
            }
            Type::GenericParam(name) => {
                if type_params.contains(name) {
                    Ok(())
                } else {
                    Err(Error::new(format!("unknown type parameter `{name}`")))
                }
            }
            Type::Apply { name, args } => {
                let expected = if name == "Result" {
                    Some(2)
                } else if let Some(decl) = self.structs.get(name) {
                    Some(decl.type_params.len())
                } else {
                    self.enums.get(name).map(|decl| decl.type_params.len())
                }
                .ok_or_else(|| Error::new(format!("unknown generic type `{name}`")))?;
                if args.len() != expected {
                    return Err(Error::new(format!(
                        "generic type `{name}` expects {expected} type argument(s), got {}",
                        args.len()
                    )));
                }
                for arg in args {
                    self.validate_type_in_scope(arg, type_params)?;
                }
                Ok(())
            }
            Type::Function(function) => {
                for param in &function.params {
                    self.validate_type_in_scope(param, type_params)?;
                }
                self.validate_type_in_scope(&function.ret, type_params)
            }
        }
    }

    fn pattern_type_arguments(
        &self,
        expected: &Type,
        variant: &VariantInfo,
        span: &Span,
    ) -> Result<Vec<Type>> {
        if variant.enum_type_params.is_empty() {
            self.expect_pattern_type_at(expected, Type::Named(variant.enum_name.clone()), span)?;
            return Ok(Vec::new());
        }
        let Type::Apply { name, args } = expected else {
            return Err(Error::diagnostic(
                Diagnostic::new("Pattern type mismatch")
                    .message("This is a type mismatch.")
                    .label(
                        span.clone(),
                        format!(
                            "This variant belongs to generic enum `{}`, but the matched value has type `{}`.",
                            variant.enum_name,
                            format_type(expected)
                        ),
                    )
                    .help("Match with a pattern from the same enum as the value being inspected."),
            ));
        };
        if name != &variant.enum_name {
            return Err(Error::diagnostic(
                Diagnostic::new("Pattern type mismatch")
                    .message("This is a type mismatch.")
                    .label(
                        span.clone(),
                        format!(
                            "This pattern belongs to `{}`, but the matched value has type `{}`.",
                            format_type(&enum_result_type(variant, args.clone())),
                            format_type(expected)
                        ),
                    )
                    .help("Use a pattern from the same enum as the matched value."),
            ));
        }
        Ok(args.clone())
    }

    fn function_value_type(&mut self, name: &str) -> Option<Type> {
        let signature = self.functions.get(name)?.clone();
        if !signature.type_params.is_empty() {
            return None;
        }
        let params = signature.param_types;
        let ret = signature.ret_type;
        Some(Type::Function(AstFunctionType {
            params,
            ret: Box::new(ret),
            effectful: signature.effectful,
        }))
    }

    fn info_at(&self, ty: usize, span: Span) -> ExprInfo {
        ExprInfo {
            ty,
            effectful: false,
            capabilities: BTreeSet::new(),
            span: Some(span),
        }
    }

    fn fresh(&mut self) -> usize {
        let id = self.types.len();
        self.types.push(TypeSlot {
            parent: id,
            value: None,
        });
        id
    }

    fn known(&mut self, ty: Type) -> usize {
        let id = self.fresh();
        self.types[id].value = Some(ty);
        id
    }

    fn find(&mut self, id: usize) -> usize {
        if self.types[id].parent != id {
            let parent = self.types[id].parent;
            let root = self.find(parent);
            self.types[id].parent = root;
        }
        self.types[id].parent
    }

    fn unify(&mut self, a: usize, b: usize) -> Result<()> {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra == rb {
            return Ok(());
        }
        match (self.types[ra].value.clone(), self.types[rb].value.clone()) {
            (Some(left), Some(right)) => {
                let merged = self.merge_known_types(&left, &right)?;
                self.types[ra].value = Some(merged);
                self.types[rb].parent = ra;
                Ok(())
            }
            (Some(_), _) => {
                self.types[rb].parent = ra;
                Ok(())
            }
            (_, Some(_)) => {
                self.types[ra].parent = rb;
                Ok(())
            }
            (None, None) => {
                self.types[rb].parent = ra;
                Ok(())
            }
        }
    }

    fn unify_at(
        &mut self,
        a: usize,
        b: usize,
        span: Span,
        message: impl Into<String>,
    ) -> Result<()> {
        let previous = self.diagnostic_context.replace((span, message.into()));
        let result = self.unify(a, b);
        self.diagnostic_context = previous;
        result
    }

    fn resolve_known(&mut self, id: usize, label: &str) -> Result<Type> {
        let root = self.find(id);
        self.types[root]
            .value
            .clone()
            .ok_or_else(|| Error::new(format!("could not infer {label}")))
    }

    fn resolve_optional(&mut self, id: usize) -> Option<Type> {
        let root = self.find(id);
        self.types[root].value.clone()
    }

    fn merge_known_types(&self, left: &Type, right: &Type) -> Result<Type> {
        if left == right {
            return Ok(left.clone());
        }
        match (left, right) {
            (Type::GenericParam(name), _) if !self.rigid_type_params.contains(name) => {
                Ok(right.clone())
            }
            (_, Type::GenericParam(name)) if !self.rigid_type_params.contains(name) => {
                Ok(left.clone())
            }
            (
                Type::Apply {
                    name: left_name,
                    args: left_args,
                },
                Type::Apply {
                    name: right_name,
                    args: right_args,
                },
            ) if left_name == right_name && left_args.len() == right_args.len() => {
                let args = left_args
                    .iter()
                    .zip(right_args.iter())
                    .map(|(left, right)| self.merge_known_types(left, right))
                    .collect::<Result<Vec<_>>>()?;
                Ok(Type::Apply {
                    name: left_name.clone(),
                    args,
                })
            }
            (Type::Function(left), Type::Function(right))
                if left.effectful == right.effectful && left.params.len() == right.params.len() =>
            {
                let params = left
                    .params
                    .iter()
                    .zip(right.params.iter())
                    .map(|(left, right)| self.merge_known_types(left, right))
                    .collect::<Result<Vec<_>>>()?;
                let ret = self.merge_known_types(&left.ret, &right.ret)?;
                Ok(Type::Function(AstFunctionType {
                    params,
                    ret: Box::new(ret),
                    effectful: left.effectful,
                }))
            }
            _ => {
                if let Some((span, message)) = &self.diagnostic_context {
                    Err(Error::diagnostic(
                        Diagnostic::new("Type mismatch")
                            .message("This is a type mismatch.")
                            .label(span.clone(), message.clone())
                            .help(format!(
                                "Expected `{}`, but found `{}`.",
                                format_type(left),
                                format_type(right)
                            )),
                    ))
                } else {
                    Err(Error::new(format!(
                        "type mismatch: expected {:?}, got {:?}",
                        left, right
                    )))
                }
            }
        }
    }

    fn placeholder_span(&self) -> Span {
        self.program
            .items
            .iter()
            .find_map(|item| match item {
                TopLevelItem::Function(function) => Some(function.body.items.first()),
                _ => None,
            })
            .flatten()
            .map(|item| match item {
                BlockItem::Binding { span, .. } => span.clone(),
                BlockItem::Expr(expr) => expr.span().clone(),
            })
            .unwrap_or_else(|| {
                let file = crate::error::SourceFile::new("<unknown>", "");
                Span::point(file, 0)
            })
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TypedProgram {
    pub(crate) functions: Vec<TypedFunction>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TypedFunction {
    pub(crate) name: String,
    pub(crate) params: Vec<Type>,
    pub(crate) ret: Type,
    pub(crate) effectful: bool,
    pub(crate) capabilities: Vec<Capability>,
}

fn format_import_path(path: &[String], name: &str) -> String {
    let mut parts = path.to_vec();
    parts.push(name.to_string());
    parts.join(".")
}

fn enum_result_type(variant: &VariantInfo, args: Vec<Type>) -> Type {
    if args.is_empty() && variant.enum_type_params.is_empty() {
        Type::Named(variant.enum_name.clone())
    } else {
        let args = if args.is_empty() {
            variant
                .enum_type_params
                .iter()
                .map(|param| Type::GenericParam(format!("{}.{}", variant.enum_name, param)))
                .collect()
        } else {
            args
        };
        Type::Apply {
            name: variant.enum_name.clone(),
            args,
        }
    }
}

fn infer_type_arguments(
    pattern: &Type,
    actual: &Type,
    substitutions: &mut HashMap<String, Type>,
) -> Result<()> {
    match pattern {
        Type::GenericParam(name) => {
            if let Some(existing) = substitutions.get(name) {
                if existing != actual {
                    if type_contains_internal_placeholder(actual) {
                        return Ok(());
                    }
                    if type_contains_internal_placeholder(existing) {
                        substitutions.insert(name.clone(), actual.clone());
                        return Ok(());
                    }
                    return Err(Error::new(format!(
                        "conflicting type argument for `{name}`: expected {:?}, got {:?}",
                        existing, actual
                    )));
                }
            } else {
                substitutions.insert(name.clone(), actual.clone());
            }
        }
        Type::Apply { name, args } => {
            if let Type::Apply {
                name: actual_name,
                args: actual_args,
            } = actual
            {
                if name == actual_name && args.len() == actual_args.len() {
                    for (left, right) in args.iter().zip(actual_args.iter()) {
                        infer_type_arguments(left, right, substitutions)?;
                    }
                }
            }
        }
        Type::Function(function) => {
            if let Type::Function(actual_function) = actual {
                for (left, right) in function.params.iter().zip(actual_function.params.iter()) {
                    infer_type_arguments(left, right, substitutions)?;
                }
                infer_type_arguments(&function.ret, &actual_function.ret, substitutions)?;
            }
        }
        Type::Prim(_) | Type::Named(_) => {}
    }
    Ok(())
}

fn substitute_type(ty: &Type, substitutions: &HashMap<String, Type>) -> Type {
    match ty {
        Type::GenericParam(name) => substitutions
            .get(name)
            .cloned()
            .unwrap_or_else(|| Type::GenericParam(name.clone())),
        Type::Apply { name, args } => Type::Apply {
            name: name.clone(),
            args: args
                .iter()
                .map(|arg| substitute_type(arg, substitutions))
                .collect(),
        },
        Type::Function(function) => Type::Function(AstFunctionType {
            params: function
                .params
                .iter()
                .map(|param| substitute_type(param, substitutions))
                .collect(),
            ret: Box::new(substitute_type(&function.ret, substitutions)),
            effectful: function.effectful,
        }),
        Type::Prim(_) | Type::Named(_) => ty.clone(),
    }
}

fn substitute_type_for_params(ty: &Type, params: &[String], args: &[Type]) -> Type {
    let substitutions = params
        .iter()
        .cloned()
        .zip(args.iter().cloned())
        .collect::<HashMap<_, _>>();
    substitute_type(ty, &substitutions)
}

fn type_contains_internal_placeholder(ty: &Type) -> bool {
    match ty {
        Type::GenericParam(name) => name.contains('.'),
        Type::Apply { args, .. } => args.iter().any(type_contains_internal_placeholder),
        Type::Function(function) => {
            function
                .params
                .iter()
                .any(type_contains_internal_placeholder)
                || type_contains_internal_placeholder(&function.ret)
        }
        Type::Prim(_) | Type::Named(_) => false,
    }
}

fn normalize_type_params(ty: &Type, params: &[String]) -> Type {
    match ty {
        Type::Named(name) if params.contains(name) => Type::GenericParam(name.clone()),
        Type::Apply { name, args } => Type::Apply {
            name: name.clone(),
            args: args
                .iter()
                .map(|arg| normalize_type_params(arg, params))
                .collect(),
        },
        Type::Function(function) => Type::Function(AstFunctionType {
            params: function
                .params
                .iter()
                .map(|param| normalize_type_params(param, params))
                .collect(),
            ret: Box::new(normalize_type_params(&function.ret, params)),
            effectful: function.effectful,
        }),
        Type::Prim(_) | Type::Named(_) | Type::GenericParam(_) => ty.clone(),
    }
}

fn collect_generic_param_mismatches(
    expected: &Type,
    actual: &Type,
    mismatches: &mut Vec<(String, String)>,
) {
    match (expected, actual) {
        (Type::GenericParam(expected), Type::GenericParam(actual)) if expected != actual => {
            mismatches.push((expected.clone(), actual.clone()));
        }
        (
            Type::Apply {
                name: expected_name,
                args: expected_args,
            },
            Type::Apply {
                name: actual_name,
                args: actual_args,
            },
        ) if expected_name == actual_name && expected_args.len() == actual_args.len() => {
            for (expected, actual) in expected_args.iter().zip(actual_args.iter()) {
                collect_generic_param_mismatches(expected, actual, mismatches);
            }
        }
        (Type::Function(expected), Type::Function(actual))
            if expected.params.len() == actual.params.len() =>
        {
            for (expected, actual) in expected.params.iter().zip(actual.params.iter()) {
                collect_generic_param_mismatches(expected, actual, mismatches);
            }
            collect_generic_param_mismatches(&expected.ret, &actual.ret, mismatches);
        }
        _ => {}
    }
}

fn function_type_return_contains_generic_param(ty: &Type, name: &str) -> bool {
    match ty {
        Type::Function(function) => type_contains_generic_param(&function.ret, name),
        Type::Apply { args, .. } => args
            .iter()
            .any(|arg| function_type_return_contains_generic_param(arg, name)),
        Type::Prim(_) | Type::Named(_) | Type::GenericParam(_) => false,
    }
}

fn type_contains_generic_param(ty: &Type, name: &str) -> bool {
    match ty {
        Type::GenericParam(param) => param == name,
        Type::Apply { args, .. } => args
            .iter()
            .any(|arg| type_contains_generic_param(arg, name)),
        Type::Function(function) => {
            function
                .params
                .iter()
                .any(|param| type_contains_generic_param(param, name))
                || type_contains_generic_param(&function.ret, name)
        }
        Type::Prim(_) | Type::Named(_) => false,
    }
}

fn format_type(ty: &Type) -> String {
    match ty {
        Type::Prim(PrimType::I32) => "I32".to_string(),
        Type::Prim(PrimType::Bool) => "Bool".to_string(),
        Type::Prim(PrimType::String) => "String".to_string(),
        Type::Prim(PrimType::Unit) => "Unit".to_string(),
        Type::Named(name) | Type::GenericParam(name) => name.clone(),
        Type::Apply { name, args } => format!(
            "{}<{}>",
            name,
            args.iter().map(format_type).collect::<Vec<_>>().join(", ")
        ),
        Type::Function(function) => {
            let bang = if function.effectful { "!" } else { "" };
            format!(
                "fn{}({}) -> {}",
                bang,
                function
                    .params
                    .iter()
                    .map(format_type)
                    .collect::<Vec<_>>()
                    .join(", "),
                format_type(&function.ret)
            )
        }
    }
}
