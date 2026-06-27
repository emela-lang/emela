use std::collections::{BTreeSet, HashMap};

use crate::ast::{
    BinaryOp, Block, BlockItem, Capability, Expr, MatchArm, Pattern, PrimType, Program,
};
use crate::error::{Error, Result};
use crate::platform::Target;

#[derive(Debug, Clone)]
struct TypeSlot {
    parent: usize,
    value: Option<PrimType>,
}

#[derive(Debug, Clone)]
struct FunctionType {
    params: Vec<usize>,
    ret: usize,
    effectful: bool,
    declared_capabilities: Option<BTreeSet<Capability>>,
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
    functions: HashMap<String, FunctionType>,
}

impl<'a> TypeChecker<'a> {
    pub(crate) fn new(program: &'a Program, target: Target) -> Self {
        Self {
            program,
            target,
            types: Vec::new(),
            functions: HashMap::new(),
        }
    }

    pub(crate) fn check(mut self) -> Result<TypedProgram> {
        self.register_functions()?;
        self.check_main()?;

        let mut function_capabilities = HashMap::new();
        for function in &self.program.functions {
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
        for function in &self.program.functions {
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

    fn register_functions(&mut self) -> Result<()> {
        for function in &self.program.functions {
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
            let ret = match function.return_annotation {
                Some(ty) => self.known(ty),
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
        let entry_count = self
            .program
            .functions
            .iter()
            .filter(|function| function.name == "main" || function.name == "main!")
            .count();
        if entry_count != 1 {
            return Err(Error::new(
                "executable program must contain exactly one top-level `main` or `main!` function",
            ));
        }
        let main = self
            .program
            .functions
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
        let entry = self
            .program
            .functions
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
                .unwrap_or_else(|| self.known(PrimType::Unit)),
            effectful,
            capabilities,
        })
    }

    fn check_expr(&mut self, expr: &Expr, scope: &mut HashMap<String, usize>) -> Result<ExprInfo> {
        match expr {
            Expr::Int(_) => {
                let ty = self.known(PrimType::I32);
                Ok(self.info(ty))
            }
            Expr::Bool(_) => {
                let ty = self.known(PrimType::Bool);
                Ok(self.info(ty))
            }
            Expr::Unit => {
                let ty = self.known(PrimType::Unit);
                Ok(self.info(ty))
            }
            Expr::Var(name) => scope
                .get(name)
                .copied()
                .map(|ty| self.info(ty))
                .ok_or_else(|| Error::new(format!("unknown local binding `{name}`"))),
            Expr::Call { name, args } => {
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
            Expr::Binary { op, left, right } => self.check_binary(*op, left, right, scope),
            Expr::Match { scrutinee, arms } => {
                if arms.is_empty() {
                    return Err(Error::new("match expression must have at least one arm"));
                }

                let scrutinee = self.check_expr(scrutinee, scope)?;
                let mut effectful = scrutinee.effectful;
                let mut capabilities = scrutinee.capabilities;
                for arm in arms {
                    if let Some(pattern_ty) = self.pattern_type(&arm.pattern) {
                        let pattern_ty = self.known(pattern_ty);
                        self.unify(scrutinee.ty, pattern_ty)?;
                    }
                }

                let mut result_ty = None;
                for arm in arms {
                    let arm = self.check_expr(&arm.expr, scope)?;
                    effectful |= arm.effectful;
                    capabilities.extend(arm.capabilities);
                    if let Some(existing) = result_ty {
                        self.unify(existing, arm.ty)?;
                    } else {
                        result_ty = Some(arm.ty);
                    }
                }

                let scrutinee_prim = self.resolve_known(scrutinee.ty, "match scrutinee type")?;
                if !self.match_is_exhaustive(scrutinee_prim, arms) {
                    return Err(Error::new("match expression is not exhaustive"));
                }

                Ok(ExprInfo {
                    ty: result_ty.expect("non-empty arms checked above"),
                    effectful,
                    capabilities,
                })
            }
            Expr::Block(block) => self.check_block(block, scope),
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
                    Some(PrimType::I32) => {
                        (Some(PrimType::I32), vec![PrimType::I32], PrimType::Bool)
                    }
                    Some(PrimType::Bool) => {
                        (Some(PrimType::Bool), vec![PrimType::Bool], PrimType::Bool)
                    }
                    Some(PrimType::Unit) => {
                        return Err(Error::new("type Unit does not implement method `eq`"));
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
                ty: self.known(ret),
                effectful,
                capabilities,
            });
        }

        if let Some(receiver_constraint) = receiver_constraint {
            let receiver_constraint = self.known(receiver_constraint);
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
            let expected_ty = self.known(*expected_ty);
            self.unify(arg.ty, expected_ty)?;
            effectful |= arg.effectful;
            capabilities.extend(arg.capabilities);
        }

        Ok(ExprInfo {
            ty: self.known(ret),
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

    fn pattern_type(&self, pattern: &Pattern) -> Option<PrimType> {
        match pattern {
            Pattern::Int(_) => Some(PrimType::I32),
            Pattern::Bool(_) => Some(PrimType::Bool),
            Pattern::Unit => Some(PrimType::Unit),
            Pattern::Wildcard => None,
        }
    }

    fn match_is_exhaustive(&self, scrutinee: PrimType, arms: &[MatchArm]) -> bool {
        if arms
            .iter()
            .any(|arm| matches!(arm.pattern, Pattern::Wildcard))
        {
            return true;
        }

        match scrutinee {
            PrimType::Bool => {
                let has_true = arms
                    .iter()
                    .any(|arm| matches!(arm.pattern, Pattern::Bool(true)));
                let has_false = arms
                    .iter()
                    .any(|arm| matches!(arm.pattern, Pattern::Bool(false)));
                has_true && has_false
            }
            PrimType::Unit => arms.iter().any(|arm| matches!(arm.pattern, Pattern::Unit)),
            PrimType::I32 => false,
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

    fn known(&mut self, prim: PrimType) -> usize {
        let id = self.fresh();
        self.types[id].value = Some(prim);
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
        match (self.types[ra].value, self.types[rb].value) {
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

    fn resolve_known(&mut self, id: usize, label: &str) -> Result<PrimType> {
        let root = self.find(id);
        self.types[root]
            .value
            .ok_or_else(|| Error::new(format!("could not infer {label}")))
    }

    fn resolve_optional(&mut self, id: usize) -> Option<PrimType> {
        let root = self.find(id);
        self.types[root].value
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TypedProgram {
    pub(crate) functions: Vec<TypedFunction>,
}

#[derive(Debug, Clone)]
pub(crate) struct TypedFunction {
    pub(crate) name: String,
    pub(crate) params: Vec<PrimType>,
    pub(crate) ret: PrimType,
    pub(crate) effectful: bool,
    pub(crate) capabilities: Vec<Capability>,
}
