//! Prints the HIR in a readable form, for snapshot tests that check where names resolve to.
//!
//! - Source definitions are `Module.name` (`Module.Parent.name` for variants, operations, type
//!   parameters and the like)
//! - Prelude definitions are the bare name (`Int`, `Some`)
//! - Built-in module definitions are `@Module.name`
//! - Locals are `name#n`, numbered in order of first appearance in the dump
//! - Unresolved names are `<error>`

use std::fmt::Write;

use rustc_hash::FxHashMap;

use crate::hir::*;
use crate::source::{ModuleId, ModuleMap};

pub fn dump_module(program: &Program, modules: &ModuleMap, module: ModuleId) -> String {
    let mut dumper = Dumper {
        program,
        modules,
        out: String::new(),
        locals: FxHashMap::default(),
    };
    if let Some(items) = program.modules.get(module) {
        for &def in &items.items {
            dumper.item(def);
        }
    }
    dumper.out
}

struct Dumper<'a> {
    program: &'a Program,
    modules: &'a ModuleMap,
    out: String,
    locals: FxHashMap<LocalId, usize>,
}

impl Dumper<'_> {
    fn line(&mut self, depth: usize, text: impl AsRef<str>) {
        let _ = writeln!(self.out, "{}{}", "  ".repeat(depth), text.as_ref());
    }

    fn def(&self, def: DefId) -> String {
        let data = &self.program.defs[def];
        if let Some(parent) = data.parent {
            let mut path = self.def(parent);
            path.push('.');
            path.push_str(&data.name);
            return path;
        }
        match &data.module {
            DefModule::Source(m) => format!("{}.{}", self.modules[*m].name, data.name),
            DefModule::Prelude => data.name.to_string(),
            DefModule::Builtin(m) => format!("@{m}.{}", data.name),
        }
    }

    fn local(&mut self, local: LocalId) -> String {
        let next = self.locals.len();
        let n = *self.locals.entry(local).or_insert(next);
        format!("{}#{n}", self.program.locals[local].name)
    }

    fn res(&mut self, res: Res) -> String {
        match res {
            Res::Def(def) => self.def(def),
            Res::Local(local) => self.local(local),
            Res::Err => "<error>".to_owned(),
        }
    }

    fn res_list(&mut self, items: &[(Res, Span)]) -> String {
        items
            .iter()
            .map(|(res, _)| self.res(*res))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Elements of an effect set. Type arguments are printed as `State[Int]`.
    fn effect_list(&mut self, items: &[EffectRef]) -> String {
        items
            .iter()
            .map(|item| {
                let res = self.res(item.res);
                format!("{res}{}", self.ty_args(&item.args))
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Empty if there are none.
    fn ty_args(&mut self, args: &[TypeId]) -> String {
        if args.is_empty() {
            return String::new();
        }
        let args: Vec<String> = args.iter().map(|&t| self.ty(t)).collect();
        format!("[{}]", args.join(", "))
    }

    fn item(&mut self, def: DefId) {
        let data = &self.program.defs[def];
        let mut flags = String::new();
        if data.is_pub {
            flags.push_str(" pub");
        }
        if data.is_opaque {
            flags.push_str(" opaque");
        }
        let name = self.def(def);
        let Some(item) = self.program.items.get(def) else {
            self.line(0, format!("{} {name}{flags}", data.kind.describe()));
            return;
        };
        match item {
            Item::Fn(f) => {
                let mut header = format!("fn {name}{flags}");
                if f.sig.is_suspend {
                    header.push_str(" suspend");
                }
                if f.is_external {
                    header.push_str(" external");
                }
                for ext in &f.externals {
                    let args: Vec<String> = std::iter::once(ext.target.to_string())
                        .chain(ext.args.iter().map(|a| format!("{a:?}")))
                        .collect();
                    header.push_str(&format!("({})", args.join(", ")));
                }
                for ext in &f.exports {
                    let args: Vec<String> = std::iter::once(ext.target.to_string())
                        .chain(ext.args.iter().map(|a| format!("{a:?}")))
                        .collect();
                    header.push_str(&format!(" export({})", args.join(", ")));
                }
                for entry in &f.entries {
                    let adapter = match entry.adapter {
                        Some(res) => self.res(res),
                        None => "?".to_owned(),
                    };
                    header.push_str(&format!(" entry({adapter})"));
                }
                self.line(0, header);
                self.fn_body(1, &f.sig, f.body);
            }
            Item::Type(t) => {
                self.line(0, format!("type {name}{flags}"));
                self.type_params(1, &t.type_params);
                self.fields(1, &t.fields);
                if !t.derives.is_empty() {
                    let derives = self.res_list(&t.derives);
                    self.line(1, format!("derive {derives}"));
                }
                if let Some(ctor) = t.ctor {
                    let ctor = self.def(ctor);
                    self.line(1, format!("ctor {ctor}"));
                }
            }
            Item::Enum(e) => {
                self.line(0, format!("enum {name}{flags}"));
                self.type_params(1, &e.type_params);
                for &variant in &e.variants {
                    let fields = match self.program.items.get(variant) {
                        Some(Item::Ctor(ctor)) => self.fields_inline(&ctor.fields),
                        _ => String::new(),
                    };
                    let variant = self.def(variant);
                    self.line(1, format!("variant {variant}{fields}"));
                }
                if !e.derives.is_empty() {
                    let derives = self.res_list(&e.derives);
                    self.line(1, format!("derive {derives}"));
                }
            }
            Item::Error(e) => {
                let fields = self.fields_inline(&e.fields);
                self.line(0, format!("error {name}{fields}{flags}"));
                if !e.derives.is_empty() {
                    let derives = self.res_list(&e.derives);
                    self.line(1, format!("derive {derives}"));
                }
            }
            Item::Const(c) => {
                let ty =
                    c.ty.map(|t| format!(": {}", self.ty(t)))
                        .unwrap_or_default();
                self.line(0, format!("const {name}{ty}{flags}"));
                self.expr(1, c.value);
            }
            Item::Effect(e) => {
                self.line(0, format!("effect {name}{flags}"));
                if let Some(label) = &e.label {
                    let default = if label.default { " default" } else { "" };
                    let base = self.res(label.base);
                    self.line(1, format!("of {base}{default}"));
                }
                self.type_params(1, &e.type_params);
                for &op in &e.ops {
                    let op_name = self.def(op);
                    let Some(Item::Op(op)) = self.program.items.get(op) else {
                        continue;
                    };
                    let suspend = if op.sig.is_suspend { " suspend" } else { "" };
                    self.line(1, format!("op {op_name}{suspend}"));
                    self.fn_body(2, &op.sig, None);
                }
            }
            Item::Handler(h) => {
                let effect = self.res(h.effect);
                let args = self.ty_args(&h.effect_args);
                self.line(0, format!("handler {name}{flags}: {effect}{args}"));
                for field in &h.fields {
                    let ty = self.ty(field.ty);
                    self.line(1, format!("field {}: {ty}", field.name));
                }
                if let Some(ctor) = h.ctor {
                    let ctor = self.def(ctor);
                    self.line(1, format!("ctor {ctor}"));
                }
                if let Some(init) = h.init {
                    self.line(1, "init");
                    self.params(2, &h.init_params);
                    self.expr(2, init);
                }
                if let Some((local, body)) = h.release {
                    let local = self.local(local);
                    self.line(1, format!("release {local}"));
                    self.expr(2, body);
                }
                for op in &h.ops {
                    let target = self.res(op.op);
                    let self_local = self.local(op.self_local);
                    let suspend = if op.is_suspend { " suspend" } else { "" };
                    self.line(
                        1,
                        format!("fn {} -> {target}{suspend} {self_local}", op.name.name),
                    );
                    self.params(2, &op.params);
                    if let Some(body) = op.body {
                        self.line(2, "body");
                        self.expr(3, body);
                    }
                }
            }
            Item::Layer(l) => {
                let members = l
                    .members
                    .iter()
                    .map(|m| {
                        let parens = if m.args.is_some() { "(..)" } else { "" };
                        format!("{}{parens}", self.res(m.res))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                self.line(0, format!("layer {name}{flags} {{ {members} }}"));
                for m in &l.members {
                    if let Some(args) = &m.args {
                        let target = self.res(m.res);
                        self.line(1, format!("args {target}"));
                        self.args(2, &args.args);
                    }
                }
            }
            Item::Trait(t) => {
                let supertraits = if t.supertraits.is_empty() {
                    String::new()
                } else {
                    format!(": {}", self.res_list(&t.supertraits))
                };
                self.line(0, format!("trait {name}{flags}{supertraits}"));
                for &f in &t.fns {
                    let fn_name = self.def(f);
                    self.line(1, format!("fn {fn_name}"));
                    if let Some(Item::TraitFn(item)) = self.program.items.get(f) {
                        self.fn_body(2, &item.sig, item.body);
                    }
                }
            }
            Item::Impl(i) => {
                let trait_ref = self.res(i.trait_ref);
                let self_ty = i.self_ty.map(|t| self.ty(t)).unwrap_or_default();
                self.line(0, format!("impl {self_ty}: {trait_ref}"));
                self.type_params(1, &i.type_params);
                for f in &i.fns {
                    let target = self.res(f.trait_fn);
                    self.line(1, format!("fn {} -> {target}", f.name.name));
                    self.params(2, &f.params);
                    if let Some(body) = f.body {
                        self.line(2, "body");
                        self.expr(3, body);
                    }
                }
            }
            Item::Ctor(_) | Item::Op(_) | Item::TraitFn(_) => {}
        }
    }

    fn fn_body(&mut self, depth: usize, sig: &FnSig, body: Option<ExprId>) {
        self.type_params(depth, &sig.type_params);
        self.params(depth, &sig.params);
        if let Some(ret) = sig.ret {
            let ret = self.ty(ret);
            self.line(depth, format!("ret {ret}"));
        }
        if let Some(fails) = &sig.fails {
            let fails = self.res_list(&fails.items);
            self.line(depth, format!("fails {{{fails}}}"));
        }
        if let Some(uses) = &sig.uses {
            let uses = self.effect_list(&uses.items);
            self.line(depth, format!("use {{{uses}}}"));
        }
        if let Some(body) = body {
            self.line(depth, "body");
            self.expr(depth + 1, body);
        }
    }

    fn type_params(&mut self, depth: usize, params: &[TypeParam]) {
        for param in params {
            let name = self.def(param.def);
            let bounds = if param.bounds.is_empty() {
                String::new()
            } else {
                format!(": {}", self.res_list(&param.bounds))
            };
            self.line(depth, format!("type-param {name}{bounds}"));
        }
    }

    fn params(&mut self, depth: usize, params: &[Param]) {
        for param in params {
            let local = self.local(param.local);
            let ty = param
                .ty
                .map(|t| format!(": {}", self.ty(t)))
                .unwrap_or_default();
            self.line(depth, format!("param {local}{ty}"));
        }
    }

    fn fields(&mut self, depth: usize, fields: &Fields) {
        match fields {
            Fields::None => {}
            Fields::Named(fields) => {
                for field in fields {
                    let ty = self.ty(field.ty);
                    self.line(depth, format!("field {}: {ty}", field.name));
                }
            }
            Fields::Positional(tys) => {
                for &t in tys {
                    let ty = self.ty(t);
                    self.line(depth, format!("field {ty}"));
                }
            }
        }
    }

    fn fields_inline(&mut self, fields: &Fields) -> String {
        match fields {
            Fields::None => String::new(),
            Fields::Named(fields) => {
                let fields: Vec<String> = fields
                    .iter()
                    .map(|f| format!("{}: {}", f.name, self.ty(f.ty)))
                    .collect();
                format!("({})", fields.join(", "))
            }
            Fields::Positional(tys) => {
                let tys: Vec<String> = tys.iter().map(|&t| self.ty(t)).collect();
                format!("({})", tys.join(", "))
            }
        }
    }

    fn ty(&mut self, ty: TypeId) -> String {
        match &self.program.types[ty].kind {
            TypeKind::Path { res, args } => {
                let mut s = self.res(*res);
                if !args.is_empty() {
                    let args: Vec<String> = args.iter().map(|&a| self.ty(a)).collect();
                    let _ = write!(s, "[{}]", args.join(", "));
                }
                s
            }
            TypeKind::SelfType => "Self".to_owned(),
            TypeKind::Unit => "()".to_owned(),
            TypeKind::Tuple(items) => {
                let items: Vec<String> = items.iter().map(|&t| self.ty(t)).collect();
                format!("({})", items.join(", "))
            }
            TypeKind::Fn {
                params,
                ret,
                fails,
                uses,
            } => {
                let params: Vec<String> = params.iter().map(|&t| self.ty(t)).collect();
                let mut s = format!("fn({})", params.join(", "));
                if let Some(ret) = ret {
                    let ret = self.ty(*ret);
                    let _ = write!(s, " -> {ret}");
                }
                if let Some(fails) = fails {
                    let fails = self.res_list(&fails.items);
                    let _ = write!(s, " fails {{{fails}}}");
                }
                if let Some(uses) = uses {
                    let uses = self.effect_list(&uses.items);
                    let _ = write!(s, " use {{{uses}}}");
                }
                s
            }
            TypeKind::ErrorSet(set) => format!("fails {{{}}}", self.res_list(&set.items)),
            TypeKind::EffectSet(set) => format!("use {{{}}}", self.effect_list(&set.items)),
            TypeKind::Missing => "missing".to_owned(),
        }
    }

    fn pat(&mut self, pat: PatId) -> String {
        match &self.program.pats[pat].kind {
            PatKind::Wild => "_".to_owned(),
            PatKind::Bind(local) => self.local(*local),
            PatKind::Lit(lit) => literal(lit),
            PatKind::Const(res) => format!("const {}", self.res(*res)),
            PatKind::Ctor { res, args, rest } => {
                let mut s = self.res(*res);
                if let Some(args) = args {
                    let mut items: Vec<String> = args
                        .iter()
                        .map(|arg| {
                            let pat = self.pat(arg.pat);
                            match &arg.label {
                                Some(label) => format!("{}: {pat}", label.name),
                                None => pat,
                            }
                        })
                        .collect();
                    if *rest {
                        items.push("..".to_owned());
                    }
                    let _ = write!(s, "({})", items.join(", "));
                }
                s
            }
            PatKind::Unit => "()".to_owned(),
            PatKind::Tuple(items) => {
                let items: Vec<String> = items.iter().map(|&p| self.pat(p)).collect();
                format!("({})", items.join(", "))
            }
            PatKind::List { elems, rest } => {
                let mut items: Vec<String> = elems.iter().map(|&p| self.pat(p)).collect();
                if let Some(rest) = rest {
                    let name = rest.binding.map(|l| self.local(l)).unwrap_or_default();
                    items.push(format!("..{name}"));
                }
                format!("[{}]", items.join(", "))
            }
            PatKind::Missing => "missing".to_owned(),
        }
    }

    fn expr(&mut self, depth: usize, expr: ExprId) {
        match &self.program.exprs[expr].kind {
            ExprKind::Missing => self.line(depth, "missing"),
            ExprKind::Lit(lit) => self.line(depth, literal(lit)),
            ExprKind::Interpolated(parts) => {
                self.line(depth, "interpolated");
                for part in parts {
                    match part {
                        StrPart::Text(text) => self.line(depth + 1, format!("{text:?}")),
                        StrPart::Interp(e) => self.expr(depth + 1, *e),
                    }
                }
            }
            ExprKind::Path(res) => {
                let res = self.res(*res);
                self.line(depth, res);
            }
            ExprKind::Field { base, name } => {
                self.line(depth, format!("field .{}", name.name));
                self.expr(depth + 1, *base);
            }
            ExprKind::Update { ctor, fields, base } => {
                self.line(depth, "update");
                self.expr(depth + 1, *ctor);
                self.args(depth + 1, fields);
                self.line(depth + 1, "..");
                self.expr(depth + 2, *base);
            }
            ExprKind::Call { callee, args } => {
                self.line(depth, "call");
                self.expr(depth + 1, *callee);
                self.args(depth + 1, args);
            }
            ExprKind::Unary { op, operand } => {
                self.line(depth, format!("{op:?}"));
                self.expr(depth + 1, *operand);
            }
            ExprKind::Binary { op, lhs, rhs } => {
                self.line(depth, format!("{op:?}"));
                self.expr(depth + 1, *lhs);
                self.expr(depth + 1, *rhs);
            }
            ExprKind::Unit => self.line(depth, "()"),
            ExprKind::Tuple(items) => {
                self.line(depth, "tuple");
                for &item in items {
                    self.expr(depth + 1, item);
                }
            }
            ExprKind::List { elems, rest } => {
                self.line(depth, "list");
                for &elem in elems {
                    self.expr(depth + 1, elem);
                }
                if let Some(rest) = rest {
                    self.line(depth + 1, "..");
                    self.expr(depth + 2, *rest);
                }
            }
            ExprKind::Block(block) => {
                self.line(depth, "block");
                for stmt in &block.stmts {
                    match stmt {
                        Stmt::Let { pat, ty, value } => {
                            let pat = self.pat(*pat);
                            let ty = ty.map(|t| format!(": {}", self.ty(t))).unwrap_or_default();
                            self.line(depth + 1, format!("let {pat}{ty} ="));
                            self.expr(depth + 2, *value);
                        }
                        Stmt::Expr(e) => self.expr(depth + 1, *e),
                    }
                }
                if let Some(tail) = block.tail {
                    self.expr(depth + 1, tail);
                }
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.line(depth, "if");
                self.expr(depth + 1, *cond);
                self.expr(depth + 1, *then_branch);
                if let Some(e) = else_branch {
                    self.line(depth, "else");
                    self.expr(depth + 1, *e);
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                self.line(depth, "match");
                self.expr(depth + 1, *scrutinee);
                self.arms(depth + 1, arms);
            }
            ExprKind::Escape { expr, arms } => {
                self.line(depth, "escape");
                self.expr(depth + 1, *expr);
                self.arms(depth + 1, arms);
            }
            ExprKind::Fail(e) => {
                self.line(depth, "fail");
                self.expr(depth + 1, *e);
            }
            ExprKind::Assert(e) => {
                self.line(depth, "assert");
                self.expr(depth + 1, *e);
            }
            ExprKind::Use { effect, args } => {
                let effect = self.res(*effect);
                let args = self.ty_args(args);
                self.line(depth, format!("use {effect}{args}"));
            }
            ExprKind::With { handlers, body } => {
                let parens = |args: &Option<SupplyArgs>| if args.is_some() { "(..)" } else { "" };
                let names = handlers
                    .iter()
                    .map(|item| {
                        let label = format!("{}{}", self.res(item.res), parens(&item.args));
                        match item.by {
                            Some((by, _)) => {
                                let by = self.res(by);
                                format!("{label} by {by}{}", parens(&item.by_init_args))
                            }
                            None => label,
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                self.line(depth, format!("with {names}"));
                for item in handlers {
                    for args in [&item.args, &item.by_init_args].into_iter().flatten() {
                        self.line(depth + 1, "args");
                        self.args(depth + 2, &args.args);
                    }
                }
                self.expr(depth + 1, *body);
            }
            ExprKind::Lambda { params, body } => {
                let params: Vec<String> = params.iter().map(|p| self.local(p.local)).collect();
                self.line(depth, format!("lambda({})", params.join(", ")));
                self.expr(depth + 1, *body);
            }
        }
    }

    fn args(&mut self, depth: usize, args: &[Arg]) {
        for arg in args {
            match &arg.label {
                Some(label) if arg.punned => {
                    self.line(depth, format!("{}: (punned)", label.name));
                    self.expr(depth + 1, arg.value);
                }
                Some(label) => {
                    self.line(depth, format!("{}:", label.name));
                    self.expr(depth + 1, arg.value);
                }
                None => self.expr(depth, arg.value),
            }
        }
    }

    fn arms(&mut self, depth: usize, arms: &[Arm]) {
        for arm in arms {
            let pat = self.pat(arm.pat);
            self.line(depth, format!("arm {pat}"));
            if let Some(guard) = arm.guard {
                self.line(depth + 1, "guard");
                self.expr(depth + 2, guard);
            }
            self.expr(depth + 1, arm.body);
        }
    }
}

fn literal(lit: &Literal) -> String {
    match lit {
        Literal::Int(n) | Literal::Float(n) => n.to_string(),
        Literal::String(s) => format!("{s:?}"),
    }
}
