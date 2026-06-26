use std::collections::HashMap;

use crate::ast::{BinaryOp, Block, BlockItem, Expr, MatchArm, Pattern, PrimType, Program};
use crate::error::{Error, Result};

#[derive(Debug, Clone)]
struct TypeSlot {
    parent: usize,
    value: Option<PrimType>,
}

#[derive(Debug, Clone)]
struct FunctionType {
    params: Vec<usize>,
    ret: usize,
}

pub(crate) struct TypeChecker<'a> {
    program: &'a Program,
    types: Vec<TypeSlot>,
    functions: HashMap<String, FunctionType>,
}

impl<'a> TypeChecker<'a> {
    pub(crate) fn new(program: &'a Program) -> Self {
        Self {
            program,
            types: Vec::new(),
            functions: HashMap::new(),
        }
    }

    pub(crate) fn check(mut self) -> Result<TypedProgram> {
        self.register_functions()?;
        self.check_main()?;

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

            let body_ty = self.check_block(&function.body, &mut scope)?;
            self.unify(signature.ret, body_ty)?;
        }

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
            let params = function.params.iter().map(|_| self.fresh()).collect();
            let ret = match function.return_annotation {
                Some(ty) => self.known(ty),
                None => self.fresh(),
            };
            self.functions
                .insert(function.name.clone(), FunctionType { params, ret });
        }
        Ok(())
    }

    fn check_main(&self) -> Result<()> {
        let main_count = self
            .program
            .functions
            .iter()
            .filter(|function| function.name == "main")
            .count();
        if main_count != 1 {
            return Err(Error::new(
                "executable program must contain exactly one top-level `main` function",
            ));
        }
        let main = self
            .program
            .functions
            .iter()
            .find(|function| function.name == "main")
            .expect("main was counted above");
        if !main.params.is_empty() {
            return Err(Error::new("`main` must take zero parameters"));
        }
        Ok(())
    }

    fn check_block(
        &mut self,
        block: &Block,
        outer_scope: &mut HashMap<String, usize>,
    ) -> Result<usize> {
        let mut scope = outer_scope.clone();
        let mut last_expr = None;
        for item in &block.items {
            match item {
                BlockItem::Binding { name, expr } => {
                    if scope.contains_key(name) {
                        return Err(Error::new(format!(
                            "duplicate binding `{name}` in the same block"
                        )));
                    }
                    let ty = self.check_expr(expr, &mut scope)?;
                    scope.insert(name.clone(), ty);
                    last_expr = None;
                }
                BlockItem::Expr(expr) => {
                    last_expr = Some(self.check_expr(expr, &mut scope)?);
                }
            }
        }
        Ok(last_expr.unwrap_or_else(|| self.known(PrimType::Unit)))
    }

    fn check_expr(&mut self, expr: &Expr, scope: &mut HashMap<String, usize>) -> Result<usize> {
        match expr {
            Expr::Int(_) => Ok(self.known(PrimType::I32)),
            Expr::Bool(_) => Ok(self.known(PrimType::Bool)),
            Expr::Unit => Ok(self.known(PrimType::Unit)),
            Expr::Var(name) => scope
                .get(name)
                .copied()
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
                for (arg, param_ty) in args.iter().zip(signature.params.iter()) {
                    let arg_ty = self.check_expr(arg, scope)?;
                    self.unify(arg_ty, *param_ty)?;
                }
                Ok(signature.ret)
            }
            Expr::MethodCall { .. } => Err(Error::new(
                "method calls are parsed but trait method resolution is not implemented yet",
            )),
            Expr::Binary { op, left, right } => {
                let left_ty = self.check_expr(left, scope)?;
                let right_ty = self.check_expr(right, scope)?;
                match op {
                    BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul => {
                        let i32_ty = self.known(PrimType::I32);
                        self.unify(left_ty, i32_ty)?;
                        let i32_ty = self.known(PrimType::I32);
                        self.unify(right_ty, i32_ty)?;
                        Ok(self.known(PrimType::I32))
                    }
                    BinaryOp::Eq => {
                        self.unify(left_ty, right_ty)?;
                        Ok(self.known(PrimType::Bool))
                    }
                    BinaryOp::Lt => {
                        let i32_ty = self.known(PrimType::I32);
                        self.unify(left_ty, i32_ty)?;
                        let i32_ty = self.known(PrimType::I32);
                        self.unify(right_ty, i32_ty)?;
                        Ok(self.known(PrimType::Bool))
                    }
                }
            }
            Expr::Match { scrutinee, arms } => {
                if arms.is_empty() {
                    return Err(Error::new("match expression must have at least one arm"));
                }

                let scrutinee_ty = self.check_expr(scrutinee, scope)?;
                for arm in arms {
                    if let Some(pattern_ty) = self.pattern_type(&arm.pattern) {
                        let pattern_ty = self.known(pattern_ty);
                        self.unify(scrutinee_ty, pattern_ty)?;
                    }
                }

                let mut result_ty = None;
                for arm in arms {
                    let arm_ty = self.check_expr(&arm.expr, scope)?;
                    if let Some(existing) = result_ty {
                        self.unify(existing, arm_ty)?;
                    } else {
                        result_ty = Some(arm_ty);
                    }
                }

                let scrutinee_prim = self.resolve_known(scrutinee_ty, "match scrutinee type")?;
                if !self.match_is_exhaustive(scrutinee_prim, arms) {
                    return Err(Error::new("match expression is not exhaustive"));
                }

                Ok(result_ty.expect("non-empty arms checked above"))
            }
            Expr::Block(block) => self.check_block(block, scope),
        }
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
}
