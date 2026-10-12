//! Core IR を読める形の文字列にする．テストのスナップショットと調べものに使う．
//!
//! 見た目は Emela に寄せるが，ソースと取り違えないように Emela にない印を付ける
//! （束縛の `let`，局所変数の `%番号`，組み込み関数の `@`，演算子の `[OpTy]`，
//! 失敗しうる呼び出しの後ろの `?`，error の enum の `error enum`，証拠の表の `use[X](ctx)` と
//! `provide(..)`，操作の呼び出しの `h.X#op(..)`）．
//! IR が区別していることは全部書き分ける．読み戻すことは考えない．
//!
//! ```text
//! enum Shapes.Shape {
//!   Circle(Float)
//!   Rect(width: Float, height: Float)
//! }
//!
//! export fn main() {
//!   let x%0 = (1 +[Int] 2)
//!   @dbg[Int](x%0)
//! }
//! ```

use std::fmt::Write;

use crate::ir::{
    Arm, AssertCond, BinOp, Callee, CtorRef, EnumDef, Expr, Lit, Local, Module, OpTy, Pat, StrPart,
    Type, UnOp, VariantFields,
};

/// モジュール全体を出す．enum を先に，関数を後に，それぞれ ID の順で並べる．
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
        writeln!(p.out, "effect {} {{ {} }}\n", def.name, def.ops.join(", ")).unwrap();
    }
    for (_, f) in m.functions.iter() {
        if f.exported {
            p.out.push_str("export ");
        }
        let params: Vec<String> = f.params.iter().map(|&l| p.local(l)).collect();
        if let Some(ext) = &f.external {
            let bindings: Vec<String> = ext
                .bindings
                .iter()
                .map(|b| format!("{}{:?}", b.target, b.args))
                .collect();
            writeln!(
                p.out,
                "external fn {}({}) = {}\n",
                f.name,
                params.join(", "),
                bindings.join(" ")
            )
            .unwrap();
            continue;
        }
        write!(p.out, "fn {}({}) ", f.name, params.join(", ")).unwrap();
        p.braced(&f.body);
        p.out.push_str("\n\n");
    }
    // 最後の空行は要らない．
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
    fn enum_def(&mut self, def: &EnumDef) {
        let params: Vec<String> = (0..def.params).map(param_name).collect();
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

    fn ty(&self, ty: &Type) -> String {
        let list = |ts: &[Type]| ts.iter().map(|t| self.ty(t)).collect::<Vec<_>>().join(", ");
        match ty {
            Type::Int => "Int".into(),
            Type::Int64 => "Int64".into(),
            Type::Float => "Float".into(),
            Type::Bool => "Bool".into(),
            Type::String => "String".into(),
            Type::Unit => "()".into(),
            Type::Never => "Never".into(),
            Type::Tuple(ts) => format!("({})", list(ts)),
            Type::List(t) => format!("List[{}]", self.ty(t)),
            Type::Enum(id, ts) if ts.is_empty() => self.m.enums[*id].name.clone(),
            Type::Enum(id, ts) => format!("{}[{}]", self.m.enums[*id].name, list(ts)),
            Type::Param(i) => param_name(*i),
            Type::Fn => "fn".into(),
        }
    }

    fn local(&self, l: Local) -> String {
        format!("{}%{}", self.m.locals[l].hint, u32::from(l.into_raw()))
    }

    /// `Shapes.Shape.Rect` のように enum とバリアントの名前をつなぐ．
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

    /// `{` と `}` で囲んだブロックとして出す．`let` の入れ子は縦に並べる．
    fn braced(&mut self, e: &Expr) {
        self.out.push('{');
        self.indent += 1;
        let mut e = e;
        while let Expr::Let { var, value, body } = e {
            self.newline();
            write!(self.out, "let {} = ", self.local(*var)).unwrap();
            self.expr(value);
            e = body;
        }
        self.newline();
        self.expr(e);
        self.indent -= 1;
        self.newline();
        self.out.push('}');
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
            Expr::Fn(f) => self.out.push_str(&self.m.functions[*f].name),
            Expr::Let { .. } => self.braced(e),
            Expr::Call {
                callee,
                args,
                fails,
            } => {
                match callee {
                    Callee::Direct(f) => self.out.push_str(&self.m.functions[*f].name),
                    // 値の呼び出しは呼ばれる側を括弧で囲み，直接の呼び出しと見分ける．
                    Callee::Indirect(f) => {
                        self.out.push('(');
                        self.expr(f);
                        self.out.push(')');
                    }
                    // 操作は `ハンドラ.エフェクト#操作` と書く．
                    Callee::Op {
                        handler,
                        effect,
                        op,
                    } => {
                        self.atom(handler);
                        let def = &self.m.effects[*effect];
                        write!(self.out, ".{}#{}", def.name, def.ops[*op]).unwrap();
                    }
                }
                self.out.push('(');
                self.exprs(args);
                self.out.push(')');
                if *fails {
                    self.out.push('?');
                }
            }
            Expr::Builtin { op, ty_args, args } => {
                write!(self.out, "@{}", op.info().path()).unwrap();
                if !ty_args.is_empty() {
                    let tys: Vec<String> = ty_args.iter().map(|t| self.ty(t)).collect();
                    write!(self.out, "[{}]", tys.join(", ")).unwrap();
                }
                self.out.push('(');
                self.exprs(args);
                self.out.push(')');
            }
            Expr::Ctor { ctor, args } => {
                self.out.push_str(&self.ctor_name(*ctor));
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
                // 添字が IR の中身．名前はあれば読むために添える．
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
            Expr::List { elems, tail } => {
                self.out.push('[');
                self.exprs(elems);
                if let Some(t) = tail {
                    if !elems.is_empty() {
                        self.out.push_str(", ");
                    }
                    self.out.push_str("..");
                    self.expr(t);
                }
                self.out.push(']');
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
                // 優先順位で読み違えないように，二項演算は毎回括弧で囲む．
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
            Expr::Lambda { params, body } => {
                let params: Vec<String> = params.iter().map(|&l| self.local(l)).collect();
                write!(self.out, "fn({}) ", params.join(", ")).unwrap();
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
                    write!(self.out, "{} = ", self.local(*v)).unwrap();
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
                    AssertCond::Bool(e) => self.expr(e),
                }
                write!(self.out, ", text: {:?})", a.text).unwrap();
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
            Expr::Handler { effect, ops } => {
                let def = &self.m.effects[*effect];
                write!(self.out, "handler {} {{", def.name).unwrap();
                self.indent += 1;
                for (name, op) in def.ops.iter().zip(ops) {
                    self.newline();
                    write!(self.out, "{name}: ").unwrap();
                    self.expr(op);
                }
                self.indent -= 1;
                self.newline();
                self.out.push('}');
            }
            Expr::With { binds, body } => {
                self.out.push_str("with {");
                self.indent += 1;
                for (var, value) in binds {
                    self.newline();
                    write!(self.out, "let {} = ", self.local(*var)).unwrap();
                    self.expr(value);
                }
                self.indent -= 1;
                self.newline();
                self.out.push_str("} ");
                self.braced(body);
            }
        }
    }

    /// フィールドの参照の左辺．変数と関数とリテラル以外は括弧で囲む．
    fn atom(&mut self, e: &Expr) {
        if matches!(e, Expr::Var(_) | Expr::Fn(_) | Expr::Lit(_)) {
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

/// 型引数の名前．`A`，`B`，… と付ける．
fn param_name(i: usize) -> String {
    char::from(b'A' + i as u8).to_string()
}

/// リテラル．Int64 には `i64` を付け，Float は必ず小数点か指数を持つ形で出す．
/// Bool は IR の組み込みなので Emela の `True` ではなく `true` と出す．
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
