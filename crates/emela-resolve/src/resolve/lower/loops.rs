//! Loop expressions (`[fn.loop]`): their lowering to a local fn called in place, and the check
//! that the name is only called in tail position of the body (E3111, E3112).

use emela_syntax::SyntaxKind::ERROR;
use emela_syntax::ast::{self, AstNode};
use rowan::TextRange;
use smol_str::SmolStr;

use super::{Binder, Lower};
use crate::diagnostic::{DiagnosticKind, NotTail};
use crate::hir::*;
use crate::resolve::{Resolver, token_name};

impl Lower<'_, '_> {
    /// `loop go(p: e, ...) sig { body }` is lowered to `{ fn go(p, ...) sig { body }⏎ go(p: e,
    /// ...) }` ([`LocalKind::Loop`]). The initial values are resolved first, in the scope outside
    /// the loop expression, so neither the name nor the parameters are visible in them. The name
    /// is visible only in the body, and the parameters in a scope inside it, so a parameter of
    /// the same name hides it, as in a local fn.
    ///
    /// A parameter written twice is kept once; the call keeps both arguments, so that type
    /// inference reports the second as an argument given twice (E3104), not as a name bound
    /// twice (E2109). An element without a name is reported by syntax validation (E1101); its
    /// expression is still resolved.
    pub(super) fn loop_expr(&mut self, lp: &ast::LoopExpr) -> ExprId {
        let node = lp.syntax();
        let range = node.text_range();
        let mut args = Vec::new();
        let mut params: Vec<(SmolStr, TextRange)> = Vec::new();
        for param in lp.params() {
            let Some(token) = param.name() else {
                if let Some(value) = param.value() {
                    self.expr(value.syntax());
                }
                continue;
            };
            let (name, name_range) = token_name(&token);
            // A type with arguments written as the value (`xs: List[Int]`) is reported by syntax
            // validation (E1101, with the help of E2106); the name is not reported again.
            let type_with_args = param.syntax().children().any(|c| c.kind() == ERROR);
            let (value, punned) = match param.value() {
                Some(_) if type_with_args => {
                    (self.alloc_expr(ExprKind::Missing, name_range), false)
                }
                Some(value) => {
                    let before = self.r.diagnostic_count();
                    let id = self.expr(value.syntax());
                    self.type_as_init(before, value.syntax().text_range(), &name);
                    (id, false)
                }
                None => (self.punned_value(&name, name_range), true),
            };
            args.push(Arg {
                label: Some(Label {
                    name: name.clone(),
                    span: self.span(name_range),
                }),
                value,
                punned,
            });
            if !params.iter().any(|(n, _)| *n == name) {
                params.push((name, name_range));
            }
        }

        let (name, name_range) = match lp.name() {
            Some(token) => token_name(&token),
            None => (SmolStr::default(), range),
        };
        let local = self.alloc_local(name.clone(), LocalKind::Loop, name_range);
        self.push_scope();
        if !name.is_empty() {
            let mut binder = Binder::default();
            binder.names.push((name.clone(), local));
            self.bind_all(binder);
        }
        self.push_scope();
        let mut binder = Binder::default();
        let params = params
            .into_iter()
            .map(|(name, range)| Param {
                local: self.bind(&mut binder, name, LocalKind::Param, range),
                ty: None,
                borrowed: None,
            })
            .collect();
        self.bind_all(binder);
        let sig = self.signature(node, Vec::new(), params, false);
        let region = self.enter_region();
        let body = match lp.body() {
            Some(b) => self.block(&b),
            None => self.alloc_expr(ExprKind::Missing, range),
        };
        self.region = region;
        self.pop_scope();
        self.pop_scope();
        if !name.is_empty() {
            self.check_loop_uses(local, &name, body);
        }

        let callee = self.alloc_expr(ExprKind::Path(Res::Local(local)), name_range);
        let call = self.alloc_expr(ExprKind::Call { callee, args }, range);
        let f = LocalFn {
            local,
            sig,
            body,
            span: self.span(range),
        };
        let block = Block {
            stmts: vec![Stmt::LocalFn(Box::new(f))],
            tail: Some(call),
        };
        self.alloc_expr(ExprKind::Block(block), range)
    }

    /// A type name written as the initial value of `param` (`loop go(i: Int)`) is E2106 with a
    /// help that says where the type comes from: the diagnostic that resolving the value
    /// reported at it is replaced.
    fn type_as_init(&mut self, before: usize, range: TextRange, param: &SmolStr) {
        for d in self.r.diagnostics_since(before) {
            if d.range != Some(range) {
                continue;
            }
            if let DiagnosticKind::WrongKind { name, found, .. } = &d.kind
                && matches!(*found, "type" | "enum" | "error" | "type parameter")
            {
                d.kind = DiagnosticKind::TypeAsLoopInit {
                    name: name.clone(),
                    found,
                    param: param.clone(),
                };
            }
        }
    }

    /// E3111 and E3112: the uses of the name `local` of a loop expression in its `body`. A call
    /// is allowed only in tail position of the body (`[fn.loop]`); everything else is reported,
    /// with the innermost construct that makes the position not a tail position.
    fn check_loop_uses(&mut self, local: LocalId, name: &SmolStr, body: ExprId) {
        let mut uses = Uses {
            program: &*self.r,
            local,
            found: Vec::new(),
        };
        uses.expr(body, None);
        for (range, kind) in uses.found {
            let kind = match kind {
                Some((reason, at)) => DiagnosticKind::LoopCallNotTail {
                    name: name.clone(),
                    reason,
                    at,
                },
                None => DiagnosticKind::LoopAsValue { name: name.clone() },
            };
            self.error(range, kind);
        }
    }
}

/// Where an expression is: `None` in tail position of the loop's body, otherwise why not and the
/// range of the construct that makes it so.
type Position = Option<(NotTail, TextRange)>;

/// The uses of the name of a loop expression found in its body: a call not in tail position with
/// its [`Position`], or the name as a value (`None`).
struct Uses<'a> {
    program: &'a Resolver<'a>,
    local: LocalId,
    found: Vec<(TextRange, Option<(NotTail, TextRange)>)>,
}

impl Uses<'_> {
    fn is_name(&self, expr: ExprId) -> bool {
        matches!(self.program.expr(expr).kind, ExprKind::Path(Res::Local(l)) if l == self.local)
    }

    /// A call to the name, at `pos`: allowed in tail position.
    fn call(&mut self, call: ExprId, pos: &Position) {
        if let Some(pos) = pos {
            let range = self.program.expr(call).span.range;
            self.found.push((range, Some(pos.clone())));
        }
    }

    fn expr(&mut self, expr: ExprId, pos: Position) {
        let program = self.program;
        let e = program.expr(expr);
        let range = e.span.range;
        let not_tail = |reason: NotTail| Some((reason, range));
        match &e.kind {
            ExprKind::Path(Res::Local(l)) if *l == self.local => self.found.push((range, None)),
            ExprKind::Missing
            | ExprKind::Lit(_)
            | ExprKind::Path(_)
            | ExprKind::Unit
            | ExprKind::Use { .. } => {}
            ExprKind::Call { callee, args } => {
                if self.is_name(*callee) {
                    self.call(expr, &pos);
                } else {
                    self.expr(*callee, not_tail(NotTail::Used));
                }
                for arg in args {
                    self.expr(arg.value, not_tail(NotTail::Argument));
                }
            }
            // `x |> go` and `x |> go(a)` are calls to the name (`[fn.pipe]`).
            ExprKind::Binary {
                op: BinaryOp::Pipe,
                lhs,
                rhs,
            } => {
                self.expr(*lhs, not_tail(NotTail::Argument));
                if self.is_name(*rhs) {
                    self.call(expr, &pos);
                } else {
                    self.expr(*rhs, pos);
                }
            }
            // `&&` and `||` are evaluated as a match: the right side is in the position of the
            // whole (`[fn.operator]`).
            ExprKind::Binary {
                op: op @ (BinaryOp::And | BinaryOp::Or),
                lhs,
                rhs,
            } => {
                self.expr(*lhs, not_tail(NotTail::Operator(binary_op(*op).into())));
                self.expr(*rhs, pos);
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let reason = || not_tail(NotTail::Operator(binary_op(*op).into()));
                self.expr(*lhs, reason());
                self.expr(*rhs, reason());
            }
            ExprKind::Unary { op, operand } => {
                let op = match op {
                    UnaryOp::Not => "!",
                    UnaryOp::Neg => "-",
                };
                self.expr(*operand, not_tail(NotTail::Operator(op.into())));
            }
            ExprKind::Block(block) => self.block(block, range, pos),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.expr(*cond, not_tail(NotTail::Used));
                self.expr(*then_branch, pos.clone());
                if let Some(e) = else_branch {
                    self.expr(*e, pos);
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                self.expr(*scrutinee, not_tail(NotTail::Used));
                for arm in arms {
                    if let Some(guard) = arm.guard {
                        self.expr(guard, not_tail(NotTail::Used));
                    }
                    self.expr(arm.body, pos.clone());
                }
            }
            ExprKind::Escape { expr: target, arms } => {
                self.expr(*target, not_tail(NotTail::EscapeTarget));
                for arm in arms {
                    if let Some(guard) = arm.guard {
                        self.expr(guard, not_tail(NotTail::Used));
                    }
                    self.expr(arm.body, not_tail(NotTail::EscapeArm));
                }
            }
            ExprKind::With { handlers, body } => {
                for item in handlers {
                    if let Some((cap, _)) = &item.cap {
                        self.expr(*cap, not_tail(NotTail::Used));
                    }
                    for args in [&item.args, &item.by_init_args].into_iter().flatten() {
                        for arg in &args.args {
                            self.expr(arg.value, not_tail(NotTail::Argument));
                        }
                    }
                }
                self.expr(*body, not_tail(NotTail::WithBody));
            }
            ExprKind::Lambda { body, .. } => self.expr(*body, not_tail(NotTail::Lambda)),
            _ => {
                for child in program.program.child_exprs(expr) {
                    self.expr(child, not_tail(NotTail::Used));
                }
            }
        }
    }

    fn block(&mut self, block: &Block, range: TextRange, pos: Position) {
        // A loop expression in the body: its initial values are evaluated before it runs, and
        // its body is the body of another function.
        if let [Stmt::LocalFn(f)] = block.stmts.as_slice()
            && self.program.local(f.local).kind == LocalKind::Loop
            && let Some(call) = block.tail
            && let ExprKind::Call { args, .. } = &self.program.expr(call).kind
        {
            for arg in args {
                self.expr(arg.value, Some((NotTail::LoopInit, range)));
            }
            let name = self.program.local(f.local).name.clone();
            self.expr(f.body, Some((NotTail::Loop(name), range)));
            return;
        }
        let mut stmts = block.stmts.iter().peekable();
        while let Some(stmt) = stmts.next() {
            let later = stmts
                .peek()
                .map(|next| self.stmt_range(next))
                .or_else(|| block.tail.map(|t| self.program.expr(t).span.range))
                .unwrap_or(range);
            match stmt {
                Stmt::Let { pat, value, .. } => {
                    let at = self.program.pat(*pat).span.range;
                    self.expr(*value, Some((NotTail::Binding, at)));
                }
                Stmt::LocalFn(f) => self.expr(f.body, Some((NotTail::LocalFn, f.span.range))),
                Stmt::Expr(e) => self.expr(*e, Some((NotTail::LaterStatement, later))),
            }
        }
        if let Some(tail) = block.tail {
            self.expr(tail, pos);
        }
    }

    fn stmt_range(&self, stmt: &Stmt) -> TextRange {
        match stmt {
            Stmt::Let { pat, value, .. } => self
                .program
                .pat(*pat)
                .span
                .range
                .cover(self.program.expr(*value).span.range),
            Stmt::LocalFn(f) => f.span.range,
            Stmt::Expr(e) => self.program.expr(*e).span.range,
        }
    }
}

fn binary_op(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Rem => "%",
        BinaryOp::Eq => "==",
        BinaryOp::Ne => "!=",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::And => "&&",
        BinaryOp::Or => "||",
        BinaryOp::Pipe => "|>",
    }
}
