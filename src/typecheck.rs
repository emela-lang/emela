use std::collections::{BTreeSet, HashMap};

use crate::ast::{
    BinaryOp, Block, BlockItem, Capability, EnumDecl, Expr, MatchArm, Pattern, PrimType, Program,
    StructDecl, TopLevelItem, Type,
};
use crate::error::{Error, Result};
use crate::platform::Target;

#[derive(Debug, Clone)]
struct TypeSlot {
    parent: usize,
    value: Option<Type>,
}

#[derive(Debug, Clone)]
struct FunctionType {
    params: Vec<usize>,
    ret: usize,
    effectful: bool,
    declared_capabilities: Option<BTreeSet<Capability>>,
}

#[derive(Debug, Clone)]
struct VariantInfo {
    enum_name: String,
    payload: Option<Type>,
}

#[derive(Debug, Clone)]
struct ExprInfo {
    ty: usize,
    effectful: bool,
    capabilities: BTreeSet<Capability>,
}

pub(crate) struct TypeChecker<'a> {
    program: &'a Program,
    target: Target,
    types: Vec<TypeSlot>,
    structs: HashMap<String, &'a StructDecl>,
    enums: HashMap<String, &'a EnumDecl>,
    variants: HashMap<String, VariantInfo>,
    functions: HashMap<String, FunctionType>,
}

impl<'a> TypeChecker<'a> {
    pub(crate) fn new(program: &'a Program, target: Target) -> Self {
        Self {
            program,
            target,
            types: Vec::new(),
            structs: HashMap::new(),
            enums: HashMap::new(),
            variants: HashMap::new(),
            functions: HashMap::new(),
        }
    }

    pub(crate) fn check(mut self) -> Result<TypedProgram> {
        self.register_types()?;
        self.register_functions()?;
        self.check_main()?;

        let functions = self.program.functions();
        let mut function_capabilities = HashMap::new();
        for function in &functions {
            let signature = self
                .functions
                .get(&function.name)
                .cloned()
                .ok_or_else(|| Error::new("internal type checker error"))?;
            let mut scope = HashMap::new();
            for (param, ty) in function.params.iter().zip(signature.params.iter()) {
                if scope.insert(param.clone(), *ty).is_some() {
                    return Err(Error::new(format!(
                        "duplicate parameter `{param}` in function `{}`",
                        function.name
                    )));
                }
            }

            let body = self.check_block(&function.body, &mut scope)?;
            self.unify(signature.ret, body.ty)?;

            if !signature.effectful && body.effectful {
                return Err(Error::new(format!(
                    "pure function `{}` cannot contain unhandled effects",
                    function.name
                )));
            }

            if let Some(declared) = &signature.declared_capabilities {
                if !body.capabilities.is_subset(declared) {
                    let missing = body
                        .capabilities
                        .difference(declared)
                        .map(|capability| format!("{capability:?}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(Error::new(format!(
                        "function `{}` uses capability outside #[requires(...)]: {missing}",
                        function.name
                    )));
                }
            }

            let function_caps = signature
                .declared_capabilities
                .clone()
                .unwrap_or(body.capabilities);
            function_capabilities.insert(function.name.clone(), function_caps);
        }

        self.check_runtime_boundary(&function_capabilities)?;

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
        for item in &self.program.items {
            match item {
                TopLevelItem::Struct(decl) => {
                    if self.structs.contains_key(&decl.name) || self.enums.contains_key(&decl.name)
                    {
                        return Err(Error::new(format!("duplicate type `{}`", decl.name)));
                    }
                    self.structs.insert(decl.name.clone(), decl);
                }
                TopLevelItem::Enum(decl) => {
                    if decl.variants.is_empty() {
                        return Err(Error::new(format!(
                            "enum `{}` must declare at least one variant",
                            decl.name
                        )));
                    }
                    if self.structs.contains_key(&decl.name) || self.enums.contains_key(&decl.name)
                    {
                        return Err(Error::new(format!("duplicate type `{}`", decl.name)));
                    }
                    for variant in &decl.variants {
                        if self.variants.contains_key(&variant.name) {
                            return Err(Error::new(format!(
                                "duplicate enum variant `{}`",
                                variant.name
                            )));
                        }
                        self.variants.insert(
                            variant.name.clone(),
                            VariantInfo {
                                enum_name: decl.name.clone(),
                                payload: variant.payload.clone(),
                            },
                        );
                    }
                    self.enums.insert(decl.name.clone(), decl);
                }
                TopLevelItem::Function(_) => {}
            }
        }

        let mut referenced = Vec::new();
        for decl in self.structs.values() {
            referenced.push(&decl.field.ty);
        }
        for decl in self.enums.values() {
            for variant in &decl.variants {
                if let Some(payload) = &variant.payload {
                    referenced.push(payload);
                }
            }
        }
        for ty in referenced {
            self.validate_type(ty)?;
        }
        Ok(())
    }

    fn register_functions(&mut self) -> Result<()> {
        for function in self.program.functions() {
            if self.functions.contains_key(&function.name) {
                return Err(Error::new(format!(
                    "duplicate top-level function `{}`",
                    function.name
                )));
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
                return Err(Error::new(format!(
                    "function `{}` requires platform capabilities and must be marked with !",
                    function.name
                )));
            }

            let params = function.params.iter().map(|_| self.fresh()).collect();
            let ret = match &function.return_annotation {
                Some(ty) => {
                    self.validate_type(ty)?;
                    self.known(ty.clone())
                }
                None => self.fresh(),
            };
            self.functions.insert(
                function.name.clone(),
                FunctionType {
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
            return Err(Error::new(
                "executable program must contain exactly one top-level `main` or `main!` function",
            ));
        }
        let main = functions
            .iter()
            .find(|function| function.name == "main" || function.name == "main!")
            .expect("entry point was counted above");
        if !main.params.is_empty() {
            return Err(Error::new("`main` and `main!` must take zero parameters"));
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
        let provided = self.target.provided_capabilities();
        if !required.is_subset(&provided) {
            let missing = required
                .difference(&provided)
                .map(|capability| format!("{capability:?}"))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(Error::new(format!(
                "target `{}` does not provide required capability: {missing}",
                self.target
            )));
        }
        Ok(())
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
                BlockItem::Binding { name, expr } => {
                    if scope.contains_key(name) {
                        return Err(Error::new(format!(
                            "duplicate binding `{name}` in the same block"
                        )));
                    }
                    let info = self.check_expr(expr, &mut scope)?;
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
                .map(|info| info.ty)
                .unwrap_or_else(|| self.known(Type::Prim(PrimType::Unit))),
            effectful,
            capabilities,
        })
    }

    fn check_expr(&mut self, expr: &Expr, scope: &mut HashMap<String, usize>) -> Result<ExprInfo> {
        match expr {
            Expr::Int(_) => {
                let ty = self.known(Type::Prim(PrimType::I32));
                Ok(self.info(ty))
            }
            Expr::Bool(_) => {
                let ty = self.known(Type::Prim(PrimType::Bool));
                Ok(self.info(ty))
            }
            Expr::Unit => {
                let ty = self.known(Type::Prim(PrimType::Unit));
                Ok(self.info(ty))
            }
            Expr::Var(name) => {
                if let Some(variant) = self.variants.get(name).cloned() {
                    if variant.payload.is_some() {
                        return Err(Error::new(format!(
                            "enum variant `{name}` requires a payload"
                        )));
                    }
                    let ty = self.known(Type::Named(variant.enum_name));
                    return Ok(self.info(ty));
                }
                scope
                    .get(name)
                    .copied()
                    .map(|ty| self.info(ty))
                    .ok_or_else(|| Error::new(format!("unknown local binding `{name}`")))
            }
            Expr::Call { name, args } => {
                if let Some(variant) = self.variants.get(name).cloned() {
                    return self.check_variant_constructor(name, &variant, args, scope);
                }

                let signature = self
                    .functions
                    .get(name)
                    .cloned()
                    .ok_or_else(|| Error::new(format!("unknown function `{name}`")))?;
                if args.len() != signature.params.len() {
                    return Err(Error::new(format!(
                        "function `{name}` expects {} argument(s), got {}",
                        signature.params.len(),
                        args.len()
                    )));
                }

                let mut effectful = signature.effectful;
                let mut capabilities = signature.declared_capabilities.clone().unwrap_or_default();
                for (arg, param_ty) in args.iter().zip(signature.params.iter()) {
                    let arg = self.check_expr(arg, scope)?;
                    self.unify(arg.ty, *param_ty)?;
                    effectful |= arg.effectful;
                    capabilities.extend(arg.capabilities);
                }
                Ok(ExprInfo {
                    ty: signature.ret,
                    effectful,
                    capabilities,
                })
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
            } => self.check_method_call(receiver, name, args, scope),
            Expr::FieldAccess { receiver, field } => {
                let receiver = self.check_expr(receiver, scope)?;
                let receiver_ty = self.resolve_known(receiver.ty, "field receiver type")?;
                let Type::Named(type_name) = receiver_ty else {
                    return Err(Error::new(format!(
                        "type {:?} does not have field `{field}`",
                        receiver_ty
                    )));
                };
                let decl = self
                    .structs
                    .get(&type_name)
                    .ok_or_else(|| Error::new(format!("type `{type_name}` has no fields")))?;
                if decl.field.name != *field {
                    return Err(Error::new(format!(
                        "struct `{type_name}` does not have field `{field}`"
                    )));
                }
                let ty = self.known(decl.field.ty.clone());
                Ok(ExprInfo {
                    ty,
                    effectful: receiver.effectful,
                    capabilities: receiver.capabilities,
                })
            }
            Expr::StructLiteral { name, field, value } => {
                let decl = self
                    .structs
                    .get(name)
                    .ok_or_else(|| Error::new(format!("unknown struct `{name}`")))?;
                if decl.field.name != *field {
                    return Err(Error::new(format!(
                        "struct `{name}` does not have field `{field}`"
                    )));
                }
                let expected_ty = decl.field.ty.clone();
                let value = self.check_expr(value, scope)?;
                let expected_ty = self.known(expected_ty);
                self.unify(value.ty, expected_ty)?;
                Ok(ExprInfo {
                    ty: self.known(Type::Named(name.clone())),
                    effectful: value.effectful,
                    capabilities: value.capabilities,
                })
            }
            Expr::Binary { op, left, right } => self.check_binary(*op, left, right, scope),
            Expr::Match { scrutinee, arms } => self.check_match(scrutinee, arms, scope),
            Expr::Block(block) => self.check_block(block, scope),
        }
    }

    fn check_variant_constructor(
        &mut self,
        name: &str,
        variant: &VariantInfo,
        args: &[Expr],
        scope: &mut HashMap<String, usize>,
    ) -> Result<ExprInfo> {
        let Some(payload_ty) = &variant.payload else {
            return Err(Error::new(format!(
                "enum variant `{name}` does not take a payload"
            )));
        };
        if args.len() != 1 {
            return Err(Error::new(format!(
                "enum variant `{name}` expects 1 payload argument, got {}",
                args.len()
            )));
        }
        let arg = self.check_expr(&args[0], scope)?;
        let expected = self.known(payload_ty.clone());
        self.unify(arg.ty, expected)?;
        Ok(ExprInfo {
            ty: self.known(Type::Named(variant.enum_name.clone())),
            effectful: arg.effectful,
            capabilities: arg.capabilities,
        })
    }

    fn check_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[MatchArm],
        scope: &mut HashMap<String, usize>,
    ) -> Result<ExprInfo> {
        if arms.is_empty() {
            return Err(Error::new("match expression must have at least one arm"));
        }

        let scrutinee = self.check_expr(scrutinee, scope)?;
        let scrutinee_ty = self.resolve_known(scrutinee.ty, "match scrutinee type")?;
        let mut effectful = scrutinee.effectful;
        let mut capabilities = scrutinee.capabilities;
        let mut result_ty = None;

        for arm in arms {
            let mut arm_scope = scope.clone();
            self.check_pattern(&arm.pattern, &scrutinee_ty, &mut arm_scope)?;
            let arm = self.check_expr(&arm.expr, &mut arm_scope)?;
            effectful |= arm.effectful;
            capabilities.extend(arm.capabilities);
            if let Some(existing) = result_ty {
                self.unify(existing, arm.ty)?;
            } else {
                result_ty = Some(arm.ty);
            }
        }

        if !self.match_is_exhaustive(&scrutinee_ty, arms) {
            return Err(Error::new("match expression is not exhaustive"));
        }

        Ok(ExprInfo {
            ty: result_ty.expect("non-empty arms checked above"),
            effectful,
            capabilities,
        })
    }

    fn check_pattern(
        &mut self,
        pattern: &Pattern,
        expected: &Type,
        scope: &mut HashMap<String, usize>,
    ) -> Result<()> {
        match pattern {
            Pattern::Int(_) => self.expect_pattern_type(expected, Type::Prim(PrimType::I32)),
            Pattern::Bool(_) => self.expect_pattern_type(expected, Type::Prim(PrimType::Bool)),
            Pattern::Unit => self.expect_pattern_type(expected, Type::Prim(PrimType::Unit)),
            Pattern::Wildcard => Ok(()),
            Pattern::Var(name) => {
                if scope.contains_key(name) {
                    return Err(Error::new(format!(
                        "pattern binding `{name}` shadows an existing binding"
                    )));
                }
                let ty = self.known(expected.clone());
                scope.insert(name.clone(), ty);
                Ok(())
            }
            Pattern::Variant { name, payload } => {
                let variant = self
                    .variants
                    .get(name)
                    .cloned()
                    .ok_or_else(|| Error::new(format!("unknown enum variant `{name}`")))?;
                self.expect_pattern_type(expected, Type::Named(variant.enum_name.clone()))?;
                match (&variant.payload, payload) {
                    (Some(payload_ty), Some(payload_pattern)) => {
                        self.check_pattern(payload_pattern, payload_ty, scope)
                    }
                    (Some(_), None) => Err(Error::new(format!(
                        "enum variant `{name}` pattern requires a payload"
                    ))),
                    (None, Some(_)) => Err(Error::new(format!(
                        "enum variant `{name}` pattern does not take a payload"
                    ))),
                    (None, None) => Ok(()),
                }
            }
        }
    }

    fn expect_pattern_type(&self, expected: &Type, actual: Type) -> Result<()> {
        if *expected == actual {
            Ok(())
        } else {
            Err(Error::new(format!(
                "type mismatch: expected {:?}, got {:?}",
                expected, actual
            )))
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
                        return Err(Error::new("type Unit does not implement method `eq`"));
                    }
                    Some(other) => {
                        return Err(Error::new(format!(
                            "type {:?} does not implement method `eq`",
                            other
                        )));
                    }
                    None => (None, Vec::new(), PrimType::Bool),
                }
            }
            _ => {
                let receiver_ty = self
                    .resolve_optional(receiver.ty)
                    .map(|ty| format!("{ty:?}"))
                    .unwrap_or_else(|| "unknown type".to_string());
                return Err(Error::new(format!(
                    "{receiver_ty} does not implement method `{name}`"
                )));
            }
        };

        if name == "eq" && receiver_constraint.is_none() {
            if args.len() != 1 {
                return Err(Error::new(format!(
                    "method `{name}` expects 1 argument(s), got {}",
                    args.len()
                )));
            }
            let arg = self.check_expr(&args[0], scope)?;
            self.unify(receiver.ty, arg.ty)?;
            effectful |= arg.effectful;
            capabilities.extend(arg.capabilities);
            return Ok(ExprInfo {
                ty: self.known(Type::Prim(ret)),
                effectful,
                capabilities,
            });
        }

        if let Some(receiver_constraint) = receiver_constraint {
            let receiver_constraint = self.known(Type::Prim(receiver_constraint));
            self.unify(receiver.ty, receiver_constraint)?;
        }

        if args.len() != expected_args.len() {
            return Err(Error::new(format!(
                "method `{name}` expects {} argument(s), got {}",
                expected_args.len(),
                args.len()
            )));
        }

        for (arg, expected_ty) in args.iter().zip(expected_args.iter()) {
            let arg = self.check_expr(arg, scope)?;
            let expected_ty = self.known(Type::Prim(*expected_ty));
            self.unify(arg.ty, expected_ty)?;
            effectful |= arg.effectful;
            capabilities.extend(arg.capabilities);
        }

        Ok(ExprInfo {
            ty: self.known(Type::Prim(ret)),
            effectful,
            capabilities,
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
            Type::Prim(PrimType::I32) => false,
            Type::Named(name) => {
                let Some(decl) = self.enums.get(name) else {
                    return false;
                };
                decl.variants.iter().all(|variant| {
                    arms.iter().any(|arm| match &arm.pattern {
                        Pattern::Variant { name, .. } => name == &variant.name,
                        _ => false,
                    })
                })
            }
        }
    }

    fn validate_type(&self, ty: &Type) -> Result<()> {
        match ty {
            Type::Prim(_) => Ok(()),
            Type::Named(name) => {
                if self.structs.contains_key(name) || self.enums.contains_key(name) {
                    Ok(())
                } else {
                    Err(Error::new(format!("unknown type `{name}`")))
                }
            }
        }
    }

    fn info(&self, ty: usize) -> ExprInfo {
        ExprInfo {
            ty,
            effectful: false,
            capabilities: BTreeSet::new(),
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
            (Some(left), Some(right)) if left != right => Err(Error::new(format!(
                "type mismatch: expected {:?}, got {:?}",
                left, right
            ))),
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
}

#[derive(Debug, Clone)]
pub(crate) struct TypedProgram {
    pub(crate) functions: Vec<TypedFunction>,
}

#[derive(Debug, Clone)]
pub(crate) struct TypedFunction {
    pub(crate) name: String,
    pub(crate) params: Vec<Type>,
    pub(crate) ret: Type,
    pub(crate) effectful: bool,
    pub(crate) capabilities: Vec<Capability>,
}
