//! Renders the Core IR as readable text, for test snapshots and investigation.
//!
//! It looks like Emela, but uses marks that Emela lacks so it is not mistaken for source: `let`
//! for bindings, `%n` for locals (with their types where bound, as in `x%0: Int`), `@` for
//! builtins, `[OpTy]` on operators, `?` after fallible calls, `!` after suspending calls,
//! `error enum` for error enums, `external enum` for `@external type`s, `use[X](ctx)` and `provide(..)` for evidence tables,
//! `h.X#op(..)` for operation calls, and `fiber#op(..)` for concurrency operations. Suspending
//! functions and lambdas, and suspend fn operations, are prefixed with `suspend`; functions and
//! lambdas whose calls may fail get `fails` after the return type, and a lambda that does not
//! suspend but whose type says a call may gets `may_suspend`. Everything the IR distinguishes is
//! written distinctly. It is not meant to be parsed back.
//!
//! Type arguments follow the name in brackets: direct calls and function values
//! (`List.map[Shape, Float](xs%1, f%2)`), constructors (`Option.None[Int]`,
//! `Option.Some[Int](1)`), builtins (`@dbg[Int](x%0)`) and operation calls
//! (`s%6.Std.Fiber.Scope[_, _]#deferred[Int]()`). A list literal is followed by its type
//! (`[1, 2]: List[Int]`), and a lambda by its return type (`fn(x%0: Int) -> Int { .. }`).
//!
//! Effects list their operations' types, and an effect with type arguments also what it
//! instantiates (`effect Std.State.State[$modify.0] = Std.State.State[A] { fn get() -> A .. }`).
//!
//! ```text
//! enum Shapes.Shape {
//!   Circle(Float)
//!   Rect(width: Float, height: Float)
//! }
//!
//! export fn main() -> Int {
//!   let x%0: Int = (1 +[Int] 2)
//!   let xs%1: List[Int] = [x%0]: List[Int]
//!   @dbg[Int](x%0)
//! }
//! ```

use std::fmt::Write;

use crate::ir::{
    Arm, AssertCond, BinOp, Callee, CtorRef, EffectDef, EnumDef, Expr, FiberOp, HostModule,
    HostRef, Lit, Local, Module, OpTy, Pat, StrPart, Type, TypeId, TypeTable, UnOp, VariantFields,
};

/// Enums first, then functions, each in ID order.
pub fn dump(m: &Module) -> String {
    let mut p = Printer {
        m,
        out: String::new(),
        indent: 0,
    };
    for (_, def) in m.enums.iter() {
        p.enum_def(def);
        p.out.push('\n');
    }
    for (_, def) in m.effects.iter() {
        p.effect_def(def);
        p.out.push('\n');
    }
    for (_, f) in m.functions.iter() {
        if f.exported {
            p.out.push_str("export ");
        }
        if f.suspends {
            p.out.push_str("suspend ");
        }
        if f.constant {
            p.out.push_str("const ");
        }
        let params: Vec<String> = f.params.iter().map(|&l| p.binder(l)).collect();
        let ty_params = if f.type_params == 0 {
            String::new()
        } else {
            let names: Vec<String> = (0..f.type_params).map(param_name).collect();
            format!("[{}]", names.join(", "))
        };
        let mut ret = p.ty(&f.ret);
        if f.fails {
            ret.push_str(" fails");
        }
        if let Some(ext) = &f.external {
            let bindings: Vec<String> = ext
                .bindings
                .iter()
                .map(|b| format!("{}[{}]", b.target, host_ref(&b.host)))
                .collect();
            writeln!(
                p.out,
                "external fn {}{ty_params}({}) -> {ret} = {}\n",
                f.name,
                params.join(", "),
                bindings.join(" ")
            )
            .unwrap();
            continue;
        }
        write!(
            p.out,
            "fn {}{ty_params}({}) -> {ret} ",
            f.name,
            params.join(", ")
        )
        .unwrap();
        p.braced(&f.body);
        p.out.push_str("\n\n");
    }
    p.out.truncate(p.out.trim_end().len());
    p.out.push('\n');
    p.out
}

struct Printer<'a> {
    m: &'a Module,
    out: String,
    indent: usize,
}

impl Printer<'_> {
    /// `effect Name = Decl[Args] { .. }`, with one operation per line. The instantiation is written
    /// only for an effect with type arguments.
    fn effect_def(&mut self, def: &EffectDef) {
        write!(self.out, "effect {}", def.name).unwrap();
        if !def.args.is_empty() {
            let args: Vec<String> = def.args.iter().map(|t| self.ty(t)).collect();
            write!(self.out, " = {}[{}]", def.decl, args.join(", ")).unwrap();
        }
        self.out.push_str(" {");
        for op in &def.ops {
            let ty_params = if op.type_params == 0 {
                String::new()
            } else {
                let names: Vec<String> = (0..op.type_params).map(param_name).collect();
                format!("[{}]", names.join(", "))
            };
            let params: Vec<String> = op.params.iter().map(|t| self.ty(t)).collect();
            write!(
                self.out,
                "\n  {}fn {}{ty_params}({}) -> {}{}",
                if op.suspend { "suspend " } else { "" },
                op.name,
                params.join(", "),
                self.ty(&op.ret),
                if op.fails { " fails" } else { "" },
            )
            .unwrap();
        }
        self.out.push_str("\n}\n");
    }

    fn enum_def(&mut self, def: &EnumDef) {
        let params: Vec<String> = (0..def.params).map(param_name).collect();
        if def.external {
            self.out.push_str("external ");
        }
        if def.error {
            self.out.push_str("error ");
        }
        if params.is_empty() {
            write!(self.out, "enum {} {{", def.name).unwrap();
        } else {
            write!(self.out, "enum {}[{}] {{", def.name, params.join(", ")).unwrap();
        }
        for v in &def.variants {
            let tys: Vec<String> = v.tys.iter().map(|t| self.ty(t)).collect();
            let fields = match &v.fields {
                VariantFields::Unit => String::new(),
                VariantFields::Positional(_) => format!("({})", tys.join(", ")),
                VariantFields::Named(names) => {
                    let fs: Vec<String> = names
                        .iter()
                        .zip(&tys)
                        .map(|(n, t)| format!("{n}: {t}"))
                        .collect();
                    format!("({})", fs.join(", "))
                }
            };
            write!(self.out, "\n  {}{fields}", v.name).unwrap();
        }
        self.out.push_str("\n}\n");
    }

    fn ty(&self, ty: &TypeId) -> String {
        type_name(self.m, &self.m.types, *ty)
    }

    fn local(&self, l: Local) -> String {
        format!("{}%{}", self.m.locals[l].hint, u32::from(l.into_raw()))
    }

    /// A local where it is bound, with its type.
    fn binder(&self, l: Local) -> String {
        format!("{}: {}", self.local(l), self.ty(&self.m.locals[l].ty))
    }

    /// Joins enum and variant names, as in `Shapes.Shape.Rect`.
    fn ctor_name(&self, c: CtorRef) -> String {
        format!(
            "{}.{}",
            self.m.enums[c.enum_id].name,
            self.m.variant(c).name
        )
    }

    fn newline(&mut self) {
        self.out.push('\n');
        for _ in 0..self.indent {
            self.out.push_str("  ");
        }
    }

    /// Prints as a block in `{` and `}`, with nested `let`s stacked vertically.
    fn braced(&mut self, e: &Expr) {
        self.out.push('{');
        self.indent += 1;
        let mut e = e;
        while let Expr::Let { var, value, body } = e {
            self.newline();
            write!(self.out, "let {} = ", self.binder(*var)).unwrap();
            self.expr(value);
            e = body;
        }
        self.newline();
        self.expr(e);
        self.indent -= 1;
        self.newline();
        self.out.push('}');
    }

    /// `[A, B]` after a name; nothing without type arguments.
    fn ty_args(&mut self, ty_args: &[TypeId]) {
        if !ty_args.is_empty() {
            let tys: Vec<String> = ty_args.iter().map(|t| self.ty(t)).collect();
            write!(self.out, "[{}]", tys.join(", ")).unwrap();
        }
    }

    fn exprs(&mut self, es: &[Expr]) {
        for (i, e) in es.iter().enumerate() {
            if i > 0 {
                self.out.push_str(", ");
            }
            self.expr(e);
        }
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Lit(lit) => self.out.push_str(&lit_text(lit)),
            Expr::Var(l) => self.out.push_str(&self.local(*l)),
            Expr::Fn { f, ty_args } => {
                self.out.push_str(&self.m.functions[*f].name);
                self.ty_args(ty_args);
            }
            Expr::Let { .. } => self.braced(e),
            Expr::Call {
                callee,
                args,
                fails,
                suspends,
            } => {
                match callee {
                    Callee::Direct { f, ty_args } => {
                        self.out.push_str(&self.m.functions[*f].name);
                        self.ty_args(ty_args);
                    }
                    // Calls to values parenthesize the callee to tell them from direct calls.
                    Callee::Indirect(f) => {
                        self.out.push('(');
                        self.expr(f);
                        self.out.push(')');
                    }
                    // An operation is written `handler.Effect#op`.
                    Callee::Op {
                        handler,
                        effect,
                        op,
                        ty_args,
                    } => {
                        self.atom(handler);
                        let def = &self.m.effects[*effect];
                        write!(self.out, ".{}#{}", def.name, def.ops[*op].name).unwrap();
                        self.ty_args(ty_args);
                    }
                }
                self.out.push('(');
                self.exprs(args);
                self.out.push(')');
                if *suspends {
                    self.out.push('!');
                }
                if *fails {
                    self.out.push('?');
                }
            }
            Expr::Builtin { op, ty_args, args } => {
                write!(self.out, "@{}", op.info().path()).unwrap();
                self.ty_args(ty_args);
                self.out.push('(');
                self.exprs(args);
                self.out.push(')');
            }
            Expr::Ctor {
                ctor,
                ty_args,
                args,
            } => {
                self.out.push_str(&self.ctor_name(*ctor));
                self.ty_args(ty_args);
                if args.is_empty() {
                    return;
                }
                self.out.push('(');
                let names = match &self.m.variant(*ctor).fields {
                    VariantFields::Named(names) => Some(names.clone()),
                    _ => None,
                };
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        self.out.push_str(", ");
                    }
                    if let Some(n) = names.as_ref().and_then(|ns| ns.get(i)) {
                        write!(self.out, "{n}: ").unwrap();
                    }
                    self.expr(a);
                }
                self.out.push(')');
            }
            Expr::Field { base, ctor, index } => {
                self.atom(base);
                // The index is what the IR holds; the name is added for readability if present.
                match &self.m.variant(*ctor).fields {
                    VariantFields::Named(names) if *index < names.len() => {
                        write!(self.out, ".{}#{index}", names[*index]).unwrap()
                    }
                    _ => write!(self.out, ".#{index}").unwrap(),
                }
            }
            Expr::Tuple(es) => {
                self.out.push('(');
                self.exprs(es);
                self.out.push(')');
            }
            Expr::List { elem, elems, tail } => {
                self.out.push('[');
                self.exprs(elems);
                if let Some(t) = tail {
                    if !elems.is_empty() {
                        self.out.push_str(", ");
                    }
                    self.out.push_str("..");
                    self.expr(t);
                }
                write!(self.out, "]: List[{}]", self.ty(elem)).unwrap();
            }
            Expr::If { cond, then, else_ } => {
                self.out.push_str("if ");
                self.expr(cond);
                self.out.push(' ');
                self.braced(then);
                self.out.push_str(" else ");
                self.braced(else_);
            }
            Expr::Match { scrutinee, arms } => {
                self.out.push_str("match ");
                self.expr(scrutinee);
                self.out.push_str(" {");
                self.indent += 1;
                for arm in arms {
                    self.newline();
                    self.arm(arm);
                }
                self.indent -= 1;
                self.newline();
                self.out.push('}');
            }
            Expr::Binary { op, ty, lhs, rhs } => {
                // Always parenthesize binary operations so precedence cannot be misread.
                self.out.push('(');
                self.expr(lhs);
                write!(self.out, " {}[{}] ", bin_op(*op), op_ty(*ty)).unwrap();
                self.expr(rhs);
                self.out.push(')');
            }
            Expr::Unary { op, ty, operand } => {
                let op = match op {
                    UnOp::Neg => "-",
                    UnOp::Not => "!",
                };
                write!(self.out, "({op}[{}] ", op_ty(*ty)).unwrap();
                self.expr(operand);
                self.out.push(')');
            }
            Expr::Lambda {
                params,
                ty,
                body,
                suspends,
            } => {
                let params: Vec<String> = params.iter().map(|&l| self.binder(l)).collect();
                if *suspends {
                    self.out.push_str("suspend ");
                }
                write!(self.out, "fn({}) -> ", params.join(", ")).unwrap();
                // The parameters show whether it takes the evidence table; the flags are written
                // only where the type says more than the lambda does.
                match self.m.ty(*ty) {
                    Type::Fn(f) => {
                        self.out.push_str(&self.ty(&f.ret));
                        if f.fails {
                            self.out.push_str(" fails");
                        }
                        if f.suspends && !*suspends {
                            self.out.push_str(" may_suspend");
                        }
                    }
                    _ => write!(self.out, "<not a function: {}>", self.ty(ty)).unwrap(),
                }
                self.out.push(' ');
                self.braced(body);
            }
            Expr::Concat(parts) => {
                self.out.push_str("concat(");
                for (i, part) in parts.iter().enumerate() {
                    if i > 0 {
                        self.out.push_str(", ");
                    }
                    match part {
                        StrPart::Lit(s) => write!(self.out, "{s:?}").unwrap(),
                        StrPart::Value(e, ty) => {
                            self.expr(e);
                            write!(self.out, " as {}", self.ty(ty)).unwrap();
                        }
                    }
                }
                self.out.push(')');
            }
            Expr::Loop { vars, inits, body } => {
                self.out.push_str("loop(");
                for (i, (v, init)) in vars.iter().zip(inits).enumerate() {
                    if i > 0 {
                        self.out.push_str(", ");
                    }
                    write!(self.out, "{} = ", self.binder(*v)).unwrap();
                    self.expr(init);
                }
                self.out.push_str(") ");
                self.braced(body);
            }
            Expr::Recur(args) => {
                self.out.push_str("recur(");
                self.exprs(args);
                self.out.push(')');
            }
            Expr::Assert(a) => {
                self.out.push_str("assert(");
                match &a.cond {
                    AssertCond::Compare {
                        op,
                        op_ty: oty,
                        ty,
                        lhs,
                        rhs,
                    } => {
                        self.expr(lhs);
                        write!(self.out, " {}[{}] ", bin_op(*op), op_ty(*oty)).unwrap();
                        self.expr(rhs);
                        write!(self.out, ", ty: {}", self.ty(ty)).unwrap();
                    }
                    AssertCond::Shown {
                        cond,
                        op,
                        ty,
                        lhs,
                        rhs,
                    } => {
                        self.expr(cond);
                        write!(self.out, ", shows ").unwrap();
                        self.expr(lhs);
                        write!(self.out, " {} ", bin_op(*op)).unwrap();
                        self.expr(rhs);
                        write!(self.out, ", ty: {}", self.ty(ty)).unwrap();
                    }
                    AssertCond::Bool(e) => self.expr(e),
                }
                write!(self.out, ", text: {:?}", a.text).unwrap();
                if let Some(location) = &a.location {
                    write!(self.out, ", at: {location:?}").unwrap();
                }
                self.out.push(')');
            }
            Expr::Fail(e) => {
                self.out.push_str("fail ");
                self.expr(e);
            }
            Expr::Escape { body, arms } => {
                self.braced(body);
                self.out.push_str(" escape {");
                self.indent += 1;
                for arm in arms {
                    self.newline();
                    self.arm(arm);
                }
                self.indent -= 1;
                self.newline();
                self.out.push('}');
            }
            Expr::Use { ctx, effect } => {
                write!(self.out, "use[{}](", self.m.effects[*effect].name).unwrap();
                self.expr(ctx);
                self.out.push(')');
            }
            Expr::Provide { base, bindings } => {
                self.out.push_str("provide(");
                if let Some(b) = base {
                    self.expr(b);
                }
                self.out.push(';');
                for (effect, h) in bindings {
                    write!(self.out, " {} = ", self.m.effects[*effect].name).unwrap();
                    self.expr(h);
                }
                self.out.push(')');
            }
            Expr::Handler { name, effect, ops } => {
                let def = &self.m.effects[*effect];
                write!(self.out, "handler {name}: {} {{", def.name).unwrap();
                self.indent += 1;
                for (op_def, op) in def.ops.iter().zip(ops) {
                    self.newline();
                    write!(self.out, "{}: ", op_def.name).unwrap();
                    self.expr(op);
                }
                self.indent -= 1;
                self.newline();
                self.out.push('}');
            }
            Expr::With { binds, body } => {
                self.out.push_str("with {");
                self.indent += 1;
                for b in binds {
                    self.newline();
                    write!(self.out, "let {} = ", self.binder(b.var)).unwrap();
                    self.expr(&b.value);
                    if let Some(r) = &b.release {
                        write!(self.out, " release {} ", r.handler).unwrap();
                        self.braced(&r.body);
                    }
                }
                self.indent -= 1;
                self.newline();
                self.out.push_str("} ");
                self.braced(body);
            }
            Expr::Fiber { op, args } => {
                write!(self.out, "fiber#{}", op.name()).unwrap();
                match op {
                    FiberOp::Await { exit } | FiberOp::Attempt { exit, .. } => {
                        write!(self.out, "[{}]", self.m.enums[*exit].name).unwrap();
                    }
                    FiberOp::CauseKind { kind } => {
                        write!(self.out, "[{}]", self.m.enums[*kind].name).unwrap();
                    }
                    _ => {}
                }
                self.out.push('(');
                self.exprs(args);
                self.out.push(')');
                if op.suspends() {
                    self.out.push('!');
                }
                if op.fails() {
                    self.out.push('?');
                }
            }
        }
    }

    /// The left side of a field access. Anything but variables, functions and literals is
    /// parenthesized.
    fn atom(&mut self, e: &Expr) {
        if matches!(e, Expr::Var(_) | Expr::Fn { .. } | Expr::Lit(_)) {
            self.expr(e);
        } else {
            self.out.push('(');
            self.expr(e);
            self.out.push(')');
        }
    }

    fn arm(&mut self, arm: &Arm) {
        let pat = self.pat(&arm.pat);
        self.out.push_str(&pat);
        if let Some(g) = &arm.guard {
            self.out.push_str(" if ");
            self.expr(g);
        }
        self.out.push_str(" -> ");
        self.expr(&arm.body);
    }

    fn pat(&self, pat: &Pat) -> String {
        let list = |ps: &[Pat]| ps.iter().map(|p| self.pat(p)).collect::<Vec<_>>();
        match pat {
            Pat::Wild => "_".into(),
            Pat::Bind(l) => self.local(*l),
            Pat::Lit(lit) => lit_text(lit),
            Pat::Ctor { ctor, fields } => {
                let name = self.ctor_name(*ctor);
                if fields.is_empty() {
                    return name;
                }
                let fs = list(fields);
                let fs: Vec<String> = match &self.m.variant(*ctor).fields {
                    VariantFields::Named(names) => names
                        .iter()
                        .zip(fs)
                        .map(|(n, p)| format!("{n}: {p}"))
                        .collect(),
                    _ => fs,
                };
                format!("{name}({})", fs.join(", "))
            }
            Pat::Tuple(ps) => format!("({})", list(ps).join(", ")),
            Pat::List { elems, rest } => {
                let mut ps = list(elems);
                if let Some(r) = rest {
                    ps.push(format!("..{}", self.pat(r)));
                }
                format!("[{}]", ps.join(", "))
            }
        }
    }
}

/// The type as the dump writes it. `types` is the module's table or an extension of it
/// (`Typer::types`).
pub(crate) fn type_name(m: &Module, types: &TypeTable, ty: TypeId) -> String {
    let list = |ts: &[TypeId]| {
        ts.iter()
            .map(|&t| type_name(m, types, t))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match &types[ty] {
        Type::Int => "Int".into(),
        Type::Int64 => "Int64".into(),
        Type::Float => "Float".into(),
        Type::Bool => "Bool".into(),
        Type::String => "String".into(),
        Type::Bytes => "Bytes".into(),
        Type::Unit => "()".into(),
        Type::Never => "Never".into(),
        Type::Tuple(ts) => format!("({})", list(ts)),
        Type::List(t) => format!("List[{}]", type_name(m, types, *t)),
        Type::Enum(id, ts) if ts.is_empty() => m.enums[*id].name.clone(),
        Type::Enum(id, ts) => format!("{}[{}]", m.enums[*id].name, list(ts)),
        Type::Param(i) => param_name(*i),
        Type::Fn(f) => {
            let mut s = format!("fn({}) -> {}", list(&f.params), type_name(m, types, f.ret));
            for (flag, name) in [
                (f.fails, " fails"),
                (f.suspends, " suspends"),
                (f.ev, " ev"),
            ] {
                if flag {
                    s.push_str(name);
                }
            }
            if let Some(sets) = f.sets {
                s.push_str(&format!(
                    " (E {}, R {})",
                    type_name(m, types, sets.errors),
                    type_name(m, types, sets.effects)
                ));
            }
            s
        }
        // `{ev, suspends, C}`: the flags, then the set type parameters.
        Type::Set(set) => {
            let mut parts: Vec<String> = [
                (set.fails, "fails"),
                (set.suspends, "suspends"),
                (set.ev, "ev"),
            ]
            .into_iter()
            .filter(|(flag, _)| *flag)
            .map(|(_, name)| name.to_owned())
            .collect();
            parts.extend(set.params.iter().map(|&p| type_name(m, types, p)));
            format!("{{{}}}", parts.join(", "))
        }
        Type::Handler(e) => format!("handler {}", m.effects[*e].name),
        Type::Evidence => "evidence".into(),
        Type::Cell(t) => format!("cell {}", type_name(m, types, *t)),
        Type::FiberScope => "fiber_scope".into(),
        Type::Any => "any".into(),
    }
}

/// Named `A`, `B`, ….
fn param_name(i: usize) -> String {
    char::from(b'A' + i as u8).to_string()
}

/// Int64 gets an `i64` suffix, and Float always has a decimal point or exponent. Bool is an IR
/// builtin, so it prints as `true`, not Emela's `True`.
fn lit_text(lit: &Lit) -> String {
    match lit {
        Lit::Int(n) => n.to_string(),
        Lit::Int64(n) => format!("{n}i64"),
        Lit::Float(x) => format!("{x:?}"),
        Lit::Bool(b) => b.to_string(),
        Lit::String(s) => format!("{s:?}"),
        Lit::Unit => "()".into(),
    }
}

fn bin_op(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
    }
}

fn op_ty(ty: OpTy) -> &'static str {
    match ty {
        OpTy::Int => "Int",
        OpTy::Int64 => "Int64",
        OpTy::Float => "Float",
        OpTy::Bool => "Bool",
        OpTy::String => "String",
        OpTy::Structural => "Structural",
    }
}

/// `"console.log"`, `"env", "sin"`, `bundled "std/clock.mjs", "now"`, `file "/app/x.mjs", "f"`.
fn host_ref(host: &HostRef) -> String {
    match host {
        HostRef::Global(name) => format!("{name:?}"),
        HostRef::Import { module, name } => {
            let module = match module {
                HostModule::Named(m) => format!("{m:?}"),
                HostModule::File(path) => format!("file {path:?}"),
                HostModule::Bundled(path) => format!("bundled {path:?}"),
            };
            format!("{module}, {name:?}")
        }
    }
}
