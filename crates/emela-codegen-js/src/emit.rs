//! Emits JS text from the Core IR.
//!
//! Expressions are emitted in two ways: `value` (returns a JS expression and may emit preceding
//! statements) and `stmt` (emits statements that pass the value to a destination). Expressions
//! that need statements (`let`, `match`, loops, ...) are assigned to a temporary and then used as
//! an expression. Emitting statements first would move the evaluation of arguments to their left
//! after them, so those left arguments are bound to temporaries first to keep left-to-right
//! evaluation order.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use emela_core::*;

use crate::{Options, RUNTIME_FILE, RuntimeMode, inline_runtime};

mod assert;
mod builtins;
mod effects;
mod errors;
mod fiber;
mod suspend;

use errors::FailTarget;

#[derive(Clone)]
enum Dest {
    Return,
    Assign(String),
}

struct LoopCtx {
    label: String,
    cells: Vec<String>,
}

pub(crate) struct Emitter<'m> {
    m: &'m Module,
    out: String,
    indent: usize,
    next_tmp: u32,
    next_label: u32,
    used: BTreeSet<&'static str>,
    loops: Vec<LoopCtx>,
    /// Enums whose show functions are to be emitted. Emitted ones stay (up to `shows_done`).
    shows: Vec<EnumId>,
    shows_done: usize,
    /// The number of type parameters of the enum show function being emitted; `None` outside one.
    show_params: Option<usize>,
    /// Failure targets, innermost escape last. Empty means the function's caller (`return`).
    fail_targets: Vec<FailTarget>,
    /// Imports of external function modules (module, name, JS local name), in emission order.
    imports: Vec<(String, String, String)>,
    /// Whether the function being emitted (top level or lambda) is a generator (may suspend,
    /// spec 16.5).
    generator: bool,
    /// JS names of the top-level functions, by `FnId` index ([`function_names`]).
    fn_names: Vec<String>,
}

pub(crate) fn emit_module(m: &Module, opts: &Options, extra_exports: &[FnId]) -> String {
    let exported = |id: FnId, f: &Function| f.exported || extra_exports.contains(&id);
    let mut e = Emitter {
        m,
        out: String::new(),
        indent: 0,
        next_tmp: 0,
        next_label: 0,
        used: BTreeSet::new(),
        loops: Vec::new(),
        shows: Vec::new(),
        shows_done: 0,
        show_params: None,
        fail_targets: Vec::new(),
        imports: Vec::new(),
        generator: false,
        fn_names: function_names(m),
    };

    // An error value carries a descriptor in `$tag` that identifies the error at run time.
    e.error_descriptors();

    // Variants without fields share a single value.
    for (id, def) in m.enums.iter() {
        if m.option == Some(id) {
            // Prelude's Option None is the same value the runtime returns.
            assert!(
                def.variants.len() == 2 && def.variants[OPTION_NONE].fields.is_empty(),
                "invalid IR: Prelude's Option has the wrong shape"
            );
            let none = e.rt("$None");
            e.line(&format!(
                "const {} = {none};",
                unit_variant_name(&def.name, &def.variants[OPTION_NONE].name)
            ));
            continue;
        }
        for (k, v) in def.variants.iter().enumerate() {
            if v.fields.is_empty() {
                let tag = e.tag(id, k);
                e.line(&format!(
                    "const {} = Object.freeze({{ $tag: {tag} }});",
                    unit_variant_name(&def.name, &v.name)
                ));
            }
        }
    }
    if !m.enums.is_empty() {
        e.line("");
    }

    for (id, f) in m.functions.iter() {
        if let Some(ext) = &f.external {
            e.external_fn(id, f, ext);
            e.line("");
            continue;
        }
        let name = e.fn_js(id).to_owned();
        if f.constant {
            // The function returns the remembered value and evaluates the body (`X$init`) only on the
            // first call (spec 5.6).
            let constant = e.rt("$constant");
            e.line(&format!(
                "const {name}$cell = {{ state: 0, value: undefined }};"
            ));
            e.line(&format!("function {name}() {{"));
            e.line(&format!(
                "  return {constant}({name}$cell, {}, {name}$init);",
                js_string(source_name(&f.name))
            ));
            e.line("}");
            e.line("");
        }
        let params: Vec<String> = f.params.iter().map(|&p| e.local(p)).collect();
        e.line(&format!(
            "{} {}{}({}) {{",
            function_keyword(f.suspends),
            name,
            if f.constant { "$init" } else { "" },
            params.join(", ")
        ));
        e.indent += 1;
        e.generator = f.suspends;
        e.stmt(&f.body, &Dest::Return);
        e.generator = false;
        e.indent -= 1;
        e.line("}");
        e.line("");
    }

    // A show function may need another enum's show function, so emit until none are left.
    while e.shows_done < e.shows.len() {
        let id = e.shows[e.shows_done];
        e.shows_done += 1;
        e.show_enum_fn(id);
        e.line("");
    }

    // The functions the host calls (`[target.js.module]`): named exports and the properties of the
    // default export.
    let mut host_named: Vec<(String, String)> = Vec::new();
    let mut host_default: Vec<(String, String)> = Vec::new();
    for (id, f) in m.functions.iter() {
        let js_exports: Vec<_> = f.host_exports.iter().filter(|x| x.target == "js").collect();
        if js_exports.is_empty() {
            continue;
        }
        let wrapper = format!("{}$host", e.fn_js(id));
        e.host_export_fn(id, f, &wrapper);
        e.line("");
        for x in js_exports {
            match x.interface {
                None => host_named.push((wrapper.clone(), x.name.clone())),
                Some(_) => host_default.push((wrapper.clone(), x.name.clone())),
            }
        }
    }

    // The functions the runtime's launcher and the test runner call (`main`, the test functions).
    let mut exports: Vec<String> = m
        .functions
        .iter()
        .filter(|&(id, f)| exported(id, f) && f.host_exports.is_empty())
        .map(|(id, f)| {
            let js = e.fn_js(id).to_owned();
            if js == f.name {
                js
            } else if is_identifier_name(&f.name) {
                format!("{js} as {}", f.name)
            } else {
                // Functions of other modules (`Util.in_util`) cannot be written as identifiers,
                // so they are exported under string names (ES2022).
                format!("{js} as {}", js_string(&f.name))
            }
        })
        .collect();
    // When a suspending function is exported, also export the driver that steps generators that
    // call it. The entry module and the test runner use it, since they do not load the runtime.
    if m.functions
        .iter()
        .any(|(id, f)| exported(id, f) && f.suspends)
    {
        exports.push(e.rt("$start").to_owned());
    }
    exports.extend(host_named.iter().map(|(js, name)| {
        if is_identifier_name(name) && name != "default" {
            format!("{js} as {name}")
        } else {
            format!("{js} as {}", js_string(name))
        }
    }));
    if !exports.is_empty() {
        e.line(&format!("export {{ {} }};", exports.join(", ")));
    }
    if !host_default.is_empty() {
        let props: Vec<String> = host_default
            .iter()
            .map(|(js, name)| format!("{}: {js}", js_string(name)))
            .collect();
        e.line(&format!("export default {{ {} }};", props.join(", ")));
    }

    let mut head = String::from("// Generated by Emela.\n");
    match &opts.runtime {
        RuntimeMode::Import(spec) => {
            if !e.used.is_empty() {
                let names: Vec<&str> = e.used.iter().copied().collect();
                let spec = spec.as_deref().unwrap_or(RUNTIME_FILE);
                let _ = writeln!(
                    head,
                    "import {{ {} }} from {};",
                    names.join(", "),
                    js_string(&format!("./{spec}"))
                );
            }
        }
        RuntimeMode::Inline => {
            head.push_str(&inline_runtime());
            head.push_str("// Generated code starts here.\n");
        }
    }
    for (module, name, local) in &e.imports {
        let imported = if is_identifier_name(name) {
            name.clone()
        } else {
            js_string(name)
        };
        let _ = writeln!(
            head,
            "import {{ {imported} as {local} }} from {};",
            js_string(&effects::module_specifier(module))
        );
    }
    head.push('\n');
    head + &e.out
}

impl Emitter<'_> {
    fn line(&mut self, s: &str) {
        if s.is_empty() {
            self.out.push('\n');
            return;
        }
        for _ in 0..self.indent {
            self.out.push_str("  ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    /// The JS name of a top-level function.
    fn fn_js(&self, f: FnId) -> &str {
        &self.fn_names[f.into_raw().into_u32() as usize]
    }

    fn rt(&mut self, name: &'static str) -> &'static str {
        self.used.insert(name);
        name
    }

    fn tmp(&mut self) -> String {
        let t = format!("$t{}", self.next_tmp);
        self.next_tmp += 1;
        t
    }

    fn label(&mut self, prefix: &str) -> String {
        let l = format!("${prefix}{}", self.next_label);
        self.next_label += 1;
        l
    }

    fn local(&self, l: Local) -> String {
        let hint: String = self.m.locals[l]
            .hint
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        format!("{hint}${}", l.into_raw().into_u32())
    }

    fn emit_dest(&mut self, dest: &Dest, v: &str) {
        match dest {
            Dest::Return => self.line(&format!("return {v};")),
            Dest::Assign(t) => self.line(&format!("{t} = {v};")),
        }
    }

    /// Evaluates `es` left to right.
    fn values_in_order(&mut self, es: &[&Expr]) -> Vec<String> {
        let last_complex = es.iter().rposition(|e| !is_simple(e));
        let mut vs = Vec::with_capacity(es.len());
        for (i, e) in es.iter().enumerate() {
            let v = self.value(e);
            if last_complex.is_some_and(|k| i < k) && !is_atom(e) {
                let t = self.tmp();
                self.line(&format!("const {t} = {v};"));
                vs.push(t);
            } else {
                vs.push(v);
            }
        }
        vs
    }

    /// Binds an expression that needs statements to a temporary and returns its name.
    fn via_tmp(&mut self, e: &Expr) -> String {
        let t = self.tmp();
        self.line(&format!("let {t};"));
        self.stmt(e, &Dest::Assign(t.clone()));
        t
    }

    fn value(&mut self, e: &Expr) -> String {
        match e {
            Expr::Lit(lit) => lit_js(lit),
            Expr::Var(l) => self.local(*l),
            Expr::Fn { f, .. } => self.fn_js(*f).to_owned(),
            Expr::Let { var, value, body } => {
                let v = self.value(value);
                let name = self.local(*var);
                self.line(&format!("const {name} = {v};"));
                self.value(body)
            }
            Expr::Call {
                callee,
                args,
                fails,
                suspends,
            } => {
                let call = self.call(callee, args, *suspends);
                if *fails { self.checked(call) } else { call }
            }
            Expr::Builtin { op, ty_args, args } => {
                let info = op.info();
                assert_eq!(
                    args.len(),
                    info.params.len(),
                    "wrong number of arguments to {op:?}"
                );
                assert_eq!(
                    ty_args.len(),
                    info.ty_params as usize,
                    "wrong number of type arguments to {op:?}"
                );
                let es: Vec<&Expr> = args.iter().collect();
                let mut vs = self.values_in_order(&es);
                if let Builtin::Dbg | Builtin::Inspect = op {
                    vs.push(self.show_fn(&ty_args[0]));
                }
                let f = self.rt(builtins::runtime_name(*op));
                format!("{f}({})", vs.join(", "))
            }
            Expr::Ctor { ctor, args, .. } => {
                let def = &self.m.enums[ctor.enum_id];
                let variant = &def.variants[ctor.variant];
                assert_eq!(
                    args.len(),
                    variant.fields.len(),
                    "wrong number of arguments to {}",
                    variant.name
                );
                if variant.fields.is_empty() {
                    return unit_variant_name(&def.name, &variant.name);
                }
                let es: Vec<&Expr> = args.iter().collect();
                let vs = self.values_in_order(&es);
                let mut s = format!("{{ $tag: {}", self.tag(ctor.enum_id, ctor.variant));
                for (i, v) in vs.iter().enumerate() {
                    let _ = write!(s, ", {}: {v}", field_key(&variant.fields, i));
                }
                s.push_str(" }");
                s
            }
            Expr::Field { base, ctor, index } => {
                let b = self.value(base);
                let fields = &self.m.variant(*ctor).fields;
                format!("{b}{}", field_access(fields, *index))
            }
            Expr::Tuple(items) => {
                let es: Vec<&Expr> = items.iter().collect();
                let vs = self.values_in_order(&es);
                format!("[{}]", vs.join(", "))
            }
            Expr::List { elems, tail, .. } => {
                let mut es: Vec<&Expr> = elems.iter().collect();
                es.extend(tail.as_deref());
                let mut vs = self.values_in_order(&es);
                let tail = tail.as_ref().map(|_| vs.pop().unwrap());
                if vs.is_empty() {
                    return tail.unwrap_or_else(|| "null".to_owned());
                }
                let list = self.rt("$list");
                match tail {
                    Some(t) => format!("{list}([{}], {t})", vs.join(", ")),
                    None => format!("{list}([{}])", vs.join(", ")),
                }
            }
            Expr::If { cond, then, else_ } if is_simple(e) => {
                let c = self.value(cond);
                let a = self.value(then);
                let b = self.value(else_);
                format!("({c} ? {a} : {b})")
            }
            Expr::Binary { op, ty, lhs, rhs } => self.binary(*op, *ty, lhs, rhs),
            Expr::Unary { op, ty, operand } => {
                let v = self.value(operand);
                match (op, ty) {
                    (UnOp::Neg, OpTy::Int) => format!("((-{v}) | 0)"),
                    (UnOp::Neg, OpTy::Int64) => format!("BigInt.asIntN(64, -{v})"),
                    (UnOp::Neg, OpTy::Float) => format!("(-{v})"),
                    (UnOp::Not, OpTy::Bool) => format!("(!{v})"),
                    _ => panic!("invalid IR: unary {op:?} cannot be applied to {ty:?}"),
                }
            }
            Expr::Lambda {
                params,
                body,
                suspends,
                ..
            } => self.lambda(params, body, *suspends),
            Expr::Concat(parts) => {
                let es: Vec<&Expr> = parts
                    .iter()
                    .filter_map(|p| match p {
                        StrPart::Value(v, _) => Some(v),
                        StrPart::Lit(_) => None,
                    })
                    .collect();
                let mut vs = self.values_in_order(&es).into_iter();
                let mut s = String::from("`");
                for p in parts {
                    match p {
                        StrPart::Lit(text) => s.push_str(&template_escape(text)),
                        StrPart::Value(_, ty) => {
                            let v = vs.next().unwrap();
                            // Interpolation embeds a String as is. Int and Int64 convert in the
                            // template to the same string as `Show`. Anything else goes through
                            // its show function.
                            let shown = match *ty {
                                TypeId::INT | TypeId::INT64 | TypeId::STRING => v,
                                _ => self.show_call(ty, &v),
                            };
                            let _ = write!(s, "${{{shown}}}");
                        }
                    }
                }
                s.push('`');
                s
            }
            Expr::If { .. } | Expr::Match { .. } | Expr::Loop { .. } | Expr::Escape { .. } => {
                self.via_tmp(e)
            }
            Expr::Fail(inner) => {
                self.fail(inner);
                // The value is never used (`fail` has type Never).
                lit_js(&Lit::Unit)
            }
            Expr::Recur(_) => panic!("invalid IR: Recur outside the tail position of a Loop"),
            Expr::Use { ctx, effect } => {
                let c = self.value(ctx);
                format!("{c}.{}", effects::effect_key(self.m, *effect))
            }
            Expr::Provide { base, bindings } => self.provide(base.as_deref(), bindings),
            Expr::Handler { name, effect, ops } => self.handler(name, *effect, ops),
            // A `with` with release evaluates its body inside `try`, so the value goes through a
            // temporary.
            Expr::With { binds, .. } if binds.iter().any(|b| b.release.is_some()) => {
                self.via_tmp(e)
            }
            Expr::With { binds, body } => {
                self.with_binds(binds);
                self.value(body)
            }
            Expr::Assert(a) => {
                self.assert(a);
                lit_js(&Lit::Unit)
            }
            Expr::Fiber { op, args } => {
                let v = self.fiber(*op, args);
                if op.fails() { self.checked(v) } else { v }
            }
        }
    }

    /// Evaluates the callee and arguments left to right. Failures are not checked. A suspending
    /// call contains `yield*` or `yield` (`suspend::call`).
    fn call(&mut self, callee: &Callee, args: &[Expr], suspends: bool) -> String {
        let mut es: Vec<&Expr> = Vec::new();
        if let Callee::Indirect(c) | Callee::Op { handler: c, .. } = callee {
            es.push(c);
        }
        es.extend(args.iter());
        let mut vs = self.values_in_order(&es);
        let f = match callee {
            Callee::Direct { f, .. } => self.fn_js(*f).to_owned(),
            Callee::Indirect(_) => vs.remove(0),
            Callee::Op { effect, op, .. } => {
                let h = vs.remove(0);
                let name = &self.m.effects[*effect].ops[*op].name;
                format!("{h}{}", effects::op_access(name))
            }
        };
        self.suspending_call(callee, &f, &vs, suspends)
    }

    /// A JS expression for the string showing `v` (a JS expression) of type `ty`.
    ///
    /// This shows the structure (the same constructor form as a derived Show). A type that reaches
    /// a written Show impl never gets here: monomorphization has already turned its
    /// interpolation, `dbg`, `inspect` and assert into calls of `Show.show`, and an assert on a type
    /// without Show into `AssertCond::Bool`.
    ///
    /// `Type::Param(i)` is only valid inside an enum show function, where it calls the show
    /// function received as `$a{i}`.
    fn show_call(&mut self, ty: &TypeId, v: &str) -> String {
        let m = self.m;
        match m.ty(*ty) {
            Type::List(elem) => {
                let f = self.show_fn(elem);
                format!("{}({v}, {f})", self.rt("$showList"))
            }
            Type::Tuple(items) => {
                let fs: Vec<String> = items.iter().map(|t| self.show_fn(t)).collect();
                format!("{}({v}, [{}])", self.rt("$showTuple"), fs.join(", "))
            }
            Type::Enum(id, args) => {
                let mut s = format!("{}({v}", self.show_enum(*id));
                for a in args {
                    let f = self.show_fn(a);
                    let _ = write!(s, ", {f}");
                }
                s.push(')');
                s
            }
            _ => format!("{}({v})", self.show_fn(ty)),
        }
    }

    fn show_fn(&mut self, ty: &TypeId) -> String {
        let m = self.m;
        let t = m.ty(*ty);
        match t {
            Type::Int => self.rt("$showInt").to_owned(),
            Type::Int64 => self.rt("$showInt64").to_owned(),
            Type::Float => self.rt("$showFloat").to_owned(),
            Type::Bool => self.rt("$showBool").to_owned(),
            Type::String => self.rt("$showString").to_owned(),
            // Monomorphization shows a Bytes through its written Show impl; this is the same text.
            Type::Bytes => self.rt("$showBytes").to_owned(),
            // A set type argument of an enum has no values, so it is never shown; the enum's show
            // function still takes a show function per type parameter.
            Type::Unit | Type::Set(_) => self.rt("$showUnit").to_owned(),
            // Never has no values, so this is never called.
            Type::Never => self.rt("$unreachable").to_owned(),
            Type::Param(i) => {
                assert!(
                    self.show_params.is_some_and(|n| *i < n),
                    "invalid IR: showing type parameter {i} is only valid inside an enum show function"
                );
                format!("$a{i}")
            }
            Type::Enum(id, args) if args.is_empty() => self.show_enum(*id),
            Type::List(_) | Type::Tuple(_) | Type::Enum(..) => {
                let body = self.show_call(ty, "$x");
                format!("(($x) => {body})")
            }
            // Only `dbg` shows a function (`[trait.builtin]`): a function type has no Show, so
            // interpolation and `inspect` of one never get here.
            Type::Fn(_) => self.rt("$showFn").to_owned(),
            Type::Handler(_) => panic!("invalid IR: capability values cannot be shown (spec 8.2)"),
            Type::Evidence | Type::Cell(_) | Type::FiberScope | Type::Any => {
                panic!("invalid IR: {t:?} values cannot be shown")
            }
        }
    }

    /// Schedules the enum's show function if it has not been emitted yet.
    fn show_enum(&mut self, id: EnumId) -> String {
        if !self.shows.contains(&id) {
            self.shows.push(id);
        }
        format!("$show${}", sanitize(&self.m.enums[id].name))
    }

    /// Type parameter show functions are received as `$a0`, `$a1`, …. Constructors are shown as
    /// written in source (`Circle(radius: 1.0)`, `Rect(1.0, 2.0)`, `Empty`).
    fn show_enum_fn(&mut self, id: EnumId) {
        let m = self.m;
        let def = &m.enums[id];
        let mut params = vec!["v".to_owned()];
        params.extend((0..def.params).map(|i| format!("$a{i}")));
        let name = self.show_enum(id);
        self.line(&format!("function {name}({}) {{", params.join(", ")));
        self.show_params = Some(def.params);
        self.indent += 1;
        let switch = def.variants.len() > 1;
        if switch {
            self.line("switch (v.$tag) {");
            self.indent += 1;
        }
        for (k, variant) in def.variants.iter().enumerate() {
            assert_eq!(
                variant.tys.len(),
                variant.fields.len(),
                "invalid IR: wrong number of field types for {}",
                variant.name
            );
            let mut pieces = Pieces::default();
            pieces.text(&variant.name);
            for (i, ty) in variant.tys.iter().enumerate() {
                pieces.text(if i == 0 { "(" } else { ", " });
                if let VariantFields::Named(names) = &variant.fields {
                    pieces.text(&format!("{}: ", names[i]));
                }
                let access = format!("v{}", field_access(&variant.fields, i));
                pieces.js(self.show_call(ty, &access));
            }
            if !variant.tys.is_empty() {
                pieces.text(")");
            }
            let expr = pieces.concat();
            if switch {
                self.line(&format!("case {k}: return {expr};"));
            } else {
                self.line(&format!("return {expr};"));
            }
        }
        if switch {
            self.indent -= 1;
            self.line("}");
            let u = self.rt("$unreachable");
            self.line(&format!("return {u}();"));
        }
        self.show_params = None;
        self.indent -= 1;
        self.line("}");
    }

    fn binary(&mut self, op: BinOp, ty: OpTy, lhs: &Expr, rhs: &Expr) -> String {
        if matches!(op, BinOp::And | BinOp::Or) {
            assert_eq!(ty, OpTy::Bool, "invalid IR: {op:?} only applies to Bool");
            let js_op = if op == BinOp::And { "&&" } else { "||" };
            if is_simple(rhs) {
                let a = self.value(lhs);
                let b = self.value(rhs);
                return format!("({a} {js_op} {b})");
            }
            // When the right side needs statements, branch on the left result before evaluating
            // it.
            let a = self.value(lhs);
            let t = self.tmp();
            self.line(&format!("let {t} = {a};"));
            let test = if op == BinOp::And {
                t.clone()
            } else {
                format!("!{t}")
            };
            self.line(&format!("if ({test}) {{"));
            self.indent += 1;
            self.stmt(rhs, &Dest::Assign(t.clone()));
            self.indent -= 1;
            self.line("}");
            return t;
        }

        let vs = self.values_in_order(&[lhs, rhs]);
        self.binary_js(op, ty, &vs[0], &vs[1])
    }

    /// `a` and `b` are evaluated JS expressions. Does not handle `&&` and `||`.
    fn binary_js(&mut self, op: BinOp, ty: OpTy, a: &str, b: &str) -> String {
        let cmp = |js: &str| format!("({a} {js} {b})");
        match (ty, op) {
            (OpTy::Int, BinOp::Add) => format!("(({a} + {b}) | 0)"),
            (OpTy::Int, BinOp::Sub) => format!("(({a} - {b}) | 0)"),
            (OpTy::Int, BinOp::Mul) => format!("Math.imul({a}, {b})"),
            (OpTy::Int, BinOp::Div) => format!("{}({a}, {b})", self.rt("$idiv")),
            (OpTy::Int, BinOp::Rem) => format!("{}({a}, {b})", self.rt("$irem")),
            (OpTy::Int64, BinOp::Add) => format!("BigInt.asIntN(64, {a} + {b})"),
            (OpTy::Int64, BinOp::Sub) => format!("BigInt.asIntN(64, {a} - {b})"),
            (OpTy::Int64, BinOp::Mul) => format!("BigInt.asIntN(64, {a} * {b})"),
            (OpTy::Int64, BinOp::Div) => format!("{}({a}, {b})", self.rt("$ldiv")),
            (OpTy::Int64, BinOp::Rem) => format!("{}({a}, {b})", self.rt("$lrem")),
            (OpTy::Float, BinOp::Add) => cmp("+"),
            (OpTy::Float, BinOp::Sub) => cmp("-"),
            (OpTy::Float, BinOp::Mul) => cmp("*"),
            (OpTy::Float, BinOp::Div) => cmp("/"),
            (OpTy::Float, BinOp::Rem) => cmp("%"),
            (OpTy::Structural, BinOp::Eq) => format!("{}({a}, {b})", self.rt("$eq")),
            (OpTy::Structural, BinOp::Ne) => format!("(!{}({a}, {b}))", self.rt("$eq")),
            (OpTy::String, BinOp::Eq) => cmp("==="),
            (OpTy::String, BinOp::Ne) => cmp("!=="),
            (OpTy::String, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge) => {
                format!("({}({a}, {b}) {} 0)", self.rt("$scmp"), cmp_op(op))
            }
            (
                OpTy::Int | OpTy::Int64 | OpTy::Float | OpTy::Bool,
                BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge,
            ) => cmp(cmp_op(op)),
            _ => panic!("invalid IR: binary {op:?} cannot be applied to {ty:?}"),
        }
    }

    fn lambda(&mut self, params: &[Local], body: &Expr, suspends: bool) -> String {
        let ps: Vec<String> = params.iter().map(|&p| self.local(p)).collect();
        let ps = ps.join(", ");
        // A lambda body is a separate function, so it does not continue enclosing loops, and its
        // failures return to the lambda's caller.
        let loops = std::mem::take(&mut self.loops);
        let fail_targets = std::mem::take(&mut self.fail_targets);
        let generator = std::mem::replace(&mut self.generator, suspends);
        let s = if suspends {
            // Arrow functions cannot `yield`, so use a generator function expression.
            let body = self.function_body(body);
            format!("(function* ({ps}) {{\n{body}}})")
        } else if is_simple(body) {
            let v = self.value(body);
            format!("(({ps}) => ({v}))")
        } else {
            let body = self.function_body(body);
            format!("(({ps}) => {{\n{body}}})")
        };
        self.loops = loops;
        self.fail_targets = fail_targets;
        self.generator = generator;
        s
    }

    /// Returns the lambda body's statements indented one level deeper, including the indentation
    /// before the closing brace.
    fn function_body(&mut self, body: &Expr) -> String {
        let outer = std::mem::take(&mut self.out);
        self.indent += 1;
        self.stmt(body, &Dest::Return);
        self.indent -= 1;
        let mut inner = std::mem::replace(&mut self.out, outer);
        for _ in 0..self.indent {
            inner.push_str("  ");
        }
        inner
    }

    fn stmt(&mut self, e: &Expr, dest: &Dest) {
        match e {
            Expr::Let { var, value, body } => {
                let v = self.value(value);
                let name = self.local(*var);
                self.line(&format!("const {name} = {v};"));
                self.stmt(body, dest);
            }
            Expr::If { cond, then, else_ } if !is_simple(e) => {
                let c = self.value(cond);
                self.line(&format!("if ({}) {{", unparen(&c)));
                self.indent += 1;
                self.stmt(then, dest);
                self.indent -= 1;
                self.line("} else {");
                self.indent += 1;
                self.stmt(else_, dest);
                self.indent -= 1;
                self.line("}");
            }
            Expr::Match { scrutinee, arms } => self.match_(scrutinee, arms, dest),
            // A call in return position can return a failure as is.
            Expr::Call {
                callee,
                args,
                fails: true,
                suspends,
            } if matches!(dest, Dest::Return) && self.fail_targets.is_empty() => {
                let call = self.call(callee, args, *suspends);
                self.emit_dest(dest, &call);
            }
            Expr::Fail(inner) => self.fail(inner),
            Expr::Escape { body, arms } => self.escape(body, arms, dest),
            Expr::With { binds, body } => self.with_(binds, body, dest),
            Expr::Loop { vars, inits, body } => {
                let es: Vec<&Expr> = inits.iter().collect();
                let vs = self.values_in_order(&es);
                let label = self.label("L");
                let cells: Vec<String> = (0..vars.len()).map(|_| self.tmp()).collect();
                if !cells.is_empty() {
                    let decls: Vec<String> = cells
                        .iter()
                        .zip(&vs)
                        .map(|(c, v)| format!("{c} = {v}"))
                        .collect();
                    self.line(&format!("let {};", decls.join(", ")));
                }
                self.line(&format!("{label}: for (;;) {{"));
                self.indent += 1;
                for (&var, cell) in vars.iter().zip(&cells) {
                    let name = self.local(var);
                    self.line(&format!("const {name} = {cell};"));
                }
                self.loops.push(LoopCtx {
                    label: label.clone(),
                    cells,
                });
                self.stmt(body, dest);
                self.loops.pop();
                if let Dest::Assign(_) = dest {
                    self.line(&format!("break {label};"));
                }
                self.indent -= 1;
                self.line("}");
            }
            Expr::Recur(args) => {
                let es: Vec<&Expr> = args.iter().collect();
                let vs = self.values_in_order(&es);
                let ctx = self.loops.last().expect("invalid IR: Recur outside a Loop");
                assert_eq!(
                    ctx.cells.len(),
                    vs.len(),
                    "invalid IR: wrong number of Recur arguments"
                );
                let assigns: Vec<String> = ctx
                    .cells
                    .iter()
                    .zip(&vs)
                    .map(|(c, v)| format!("{c} = {v};"))
                    .collect();
                let label = ctx.label.clone();
                for a in assigns {
                    self.line(&a);
                }
                self.line(&format!("continue {label};"));
            }
            _ => {
                let v = self.value(e);
                self.emit_dest(dest, &v);
            }
        }
    }

    fn match_(&mut self, scrutinee: &Expr, arms: &[Arm], dest: &Dest) {
        let v = self.value(scrutinee);
        let s = if matches!(scrutinee, Expr::Var(_) | Expr::Lit(_)) {
            v
        } else {
            let t = self.tmp();
            self.line(&format!("const {t} = {v};"));
            t
        };

        // Unless the destination is return, wrap in a labeled block so the arms can break out.
        let label = match dest {
            Dest::Return => None,
            Dest::Assign(_) => {
                let l = self.label("m");
                self.line(&format!("{l}: {{"));
                self.indent += 1;
                Some(l)
            }
        };

        let mut exhaustive = false;
        for arm in arms {
            let mut conds = Vec::new();
            let mut binds = Vec::new();
            self.pattern(&arm.pat, s.clone(), &mut conds, &mut binds);
            let mut opened = 0;
            if !conds.is_empty() {
                self.line(&format!("if ({}) {{", conds.join(" && ")));
                self.indent += 1;
                opened += 1;
            }
            for (l, access) in binds {
                let name = self.local(l);
                self.line(&format!("const {name} = {access};"));
            }
            if let Some(g) = &arm.guard {
                let gv = self.value(g);
                self.line(&format!("if ({}) {{", unparen(&gv)));
                self.indent += 1;
                opened += 1;
            }
            self.stmt(&arm.body, dest);
            if let Some(l) = &label {
                self.line(&format!("break {l};"));
            }
            for _ in 0..opened {
                self.indent -= 1;
                self.line("}");
            }
            if opened == 0 {
                // Arms after an irrefutable arm are unreachable.
                exhaustive = true;
                break;
            }
        }
        if !exhaustive {
            let u = self.rt("$unreachable");
            self.line(&format!("{u}();"));
        }

        if label.is_some() {
            self.indent -= 1;
            self.line("}");
        }
    }

    /// The conditions for `access` matching `p`, and the variables it binds. The conditions are
    /// ordered so they can be evaluated left to right.
    fn pattern(
        &self,
        p: &Pat,
        access: String,
        conds: &mut Vec<String>,
        binds: &mut Vec<(Local, String)>,
    ) {
        match p {
            Pat::Wild | Pat::Lit(Lit::Unit) => {}
            Pat::Bind(l) => binds.push((*l, access)),
            Pat::Lit(lit) => conds.push(format!("{access} === {}", lit_js(lit))),
            Pat::Ctor { ctor, fields } => {
                let def = &self.m.enums[ctor.enum_id];
                let variant = &def.variants[ctor.variant];
                assert_eq!(
                    fields.len(),
                    variant.fields.len(),
                    "wrong number of fields for {}",
                    variant.name
                );
                if def.variants.len() > 1 {
                    conds.push(format!("{access}.$tag === {}", ctor.variant));
                }
                for (i, f) in fields.iter().enumerate() {
                    let sub = format!("{access}{}", field_access(&variant.fields, i));
                    self.pattern(f, sub, conds, binds);
                }
            }
            Pat::Tuple(items) => {
                for (i, item) in items.iter().enumerate() {
                    self.pattern(item, format!("{access}[{i}]"), conds, binds);
                }
            }
            Pat::List { elems, rest } => {
                let mut cur = access;
                for elem in elems {
                    conds.push(format!("{cur} !== null"));
                    self.pattern(elem, format!("{cur}.h"), conds, binds);
                    cur = format!("{cur}.t");
                }
                match rest {
                    None => conds.push(format!("{cur} === null")),
                    Some(r) => self.pattern(r, cur, conds, binds),
                }
            }
        }
    }
}

/// Parts of a string concatenation. Adjacent string literals are merged into one.
#[derive(Default)]
struct Pieces(Vec<(bool, String)>);

impl Pieces {
    fn text(&mut self, t: &str) {
        match self.0.last_mut() {
            Some((true, last)) => last.push_str(t),
            _ => self.0.push((true, t.to_owned())),
        }
    }

    fn js(&mut self, e: String) {
        self.0.push((false, e));
    }

    fn concat(self) -> String {
        let parts: Vec<String> = self
            .0
            .into_iter()
            .map(|(is_text, s)| if is_text { js_string(&s) } else { s })
            .collect();
        parts.join(" + ")
    }
}

/// Whether it can be emitted as a single JS expression with no statements.
fn is_simple(e: &Expr) -> bool {
    match e {
        Expr::Lit(_) | Expr::Var(_) | Expr::Fn { .. } | Expr::Lambda { .. } => true,
        Expr::Let { .. }
        | Expr::Match { .. }
        | Expr::Loop { .. }
        | Expr::Recur(_)
        | Expr::Assert(_)
        | Expr::Fail(_)
        | Expr::Escape { .. }
        | Expr::With { .. } => false,
        Expr::Use { ctx, .. } => is_simple(ctx),
        Expr::Provide { base, bindings } => {
            base.as_deref().is_none_or(is_simple) && bindings.iter().all(|(_, h)| is_simple(h))
        }
        Expr::Handler { ops, .. } => ops.iter().all(is_simple),
        // A fallible call emits a statement that checks its result.
        Expr::Call { fails: true, .. } => false,
        Expr::Fiber { op, args } => !op.fails() && args.iter().all(is_simple),
        Expr::Call { callee, args, .. } => {
            let callee_simple = match callee {
                Callee::Direct { .. } => true,
                Callee::Indirect(c) | Callee::Op { handler: c, .. } => is_simple(c),
            };
            callee_simple && args.iter().all(is_simple)
        }
        Expr::Builtin { args, .. } | Expr::Ctor { args, .. } | Expr::Tuple(args) => {
            args.iter().all(is_simple)
        }
        Expr::Field { base, .. } => is_simple(base),
        Expr::List { elems, tail, .. } => {
            elems.iter().all(is_simple) && tail.as_deref().is_none_or(is_simple)
        }
        Expr::If { cond, then, else_ } => is_simple(cond) && is_simple(then) && is_simple(else_),
        Expr::Binary { lhs, rhs, .. } => is_simple(lhs) && is_simple(rhs),
        Expr::Unary { operand, .. } => is_simple(operand),
        Expr::Concat(parts) => parts.iter().all(|p| match p {
            StrPart::Lit(_) => true,
            StrPart::Value(v, _) => is_simple(v),
        }),
    }
}

/// Suspending functions become generators (spec 16.5).
pub(crate) fn function_keyword(suspends: bool) -> &'static str {
    if suspends { "function*" } else { "function" }
}

/// Evaluating it has no effect and always yields the same value.
fn is_atom(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Lit(_) | Expr::Var(_) | Expr::Fn { .. } | Expr::Lambda { .. }
    )
}

/// Strips one pair of parentheses around the whole expression (for readable `if` conditions).
/// Not done when it contains a string, since parentheses could be miscounted.
fn unparen(s: &str) -> &str {
    let Some(inner) = s.strip_prefix('(').and_then(|t| t.strip_suffix(')')) else {
        return s;
    };
    if inner.contains(['"', '`']) {
        return s;
    }
    let mut depth = 0i32;
    for c in inner.chars() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return s;
                }
            }
            _ => {}
        }
    }
    if depth == 0 { inner } else { s }
}

fn cmp_op(op: BinOp) -> &'static str {
    match op {
        BinOp::Eq => "===",
        BinOp::Ne => "!==",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        _ => unreachable!(),
    }
}

fn lit_js(lit: &Lit) -> String {
    match lit {
        Lit::Int(n) if *n < 0 => format!("({n})"),
        Lit::Int(n) => n.to_string(),
        Lit::Int64(n) if *n < 0 => format!("({n}n)"),
        Lit::Int64(n) => format!("{n}n"),
        Lit::Float(x) if x.is_nan() => "NaN".to_owned(),
        Lit::Float(x) if x.is_infinite() => {
            if *x > 0.0 {
                "Infinity".to_owned()
            } else {
                "(-Infinity)".to_owned()
            }
        }
        Lit::Float(x) if x.is_sign_negative() => format!("({x:?})"),
        Lit::Float(x) => format!("{x:?}"),
        Lit::Bool(b) => b.to_string(),
        Lit::String(s) => js_string(s),
        Lit::Unit => "undefined".to_owned(),
    }
}

pub(crate) fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn template_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '`' => out.push_str("\\`"),
            '\\' => out.push_str("\\\\"),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

fn field_key(fields: &VariantFields, i: usize) -> String {
    match fields {
        VariantFields::Named(names) if names[i] == "__proto__" => "[\"__proto__\"]".to_owned(),
        VariantFields::Named(names) => names[i].clone(),
        _ => format!("${i}"),
    }
}

fn field_access(fields: &VariantFields, i: usize) -> String {
    match fields {
        VariantFields::Named(names) if names[i] == "__proto__" => "[\"__proto__\"]".to_owned(),
        VariantFields::Named(names) => format!(".{}", names[i]),
        _ => format!(".${i}"),
    }
}

fn unit_variant_name(enum_name: &str, variant: &str) -> String {
    format!("{}${}", sanitize(enum_name), sanitize(variant))
}

pub(crate) fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Whether the name can be written as an identifier in an export (ASCII only; reserved words are
/// allowed as export names).
pub(crate) fn is_identifier_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// The JS names of the module's top-level functions, by `FnId` index, all different (and different
/// from the shared values of variants without fields).
///
/// A copy made by monomorphization (`List.map[Int, String, {}, {}]`, `wrap[Int]#2`) gets the JS
/// name of the original, a `$`, and the words of its type arguments joined by `_`
/// (`List_map$Int_String`, `wrap$Int_2`): readable, and the `$` keeps it apart from the names of
/// functions without type arguments. Names that still clash get `$$2`, `$$3`, …. Locals are
/// `hint$<digits>` with a hint without `$` ([`Emitter::local`]), so a function name never equals
/// one: it has no `$`, or a `$` followed by something other than digits, or two `$`s.
fn function_names(m: &Module) -> Vec<String> {
    let mut used: std::collections::HashSet<String> = m
        .enums
        .iter()
        .flat_map(|(_, def)| {
            def.variants
                .iter()
                .filter(|v| v.fields.is_empty())
                .map(|v| unit_variant_name(&def.name, &v.name))
        })
        .collect();
    m.functions
        .iter()
        .map(|(_, f)| {
            let base = match f.name.split_once('[') {
                None => fn_name(&f.name),
                Some((name, args)) => {
                    let words: Vec<&str> = args
                        .split(|c: char| !c.is_ascii_alphanumeric())
                        .filter(|w| !w.is_empty())
                        .collect();
                    let mut args = words.join("_");
                    if args.starts_with(|c: char| c.is_ascii_digit()) {
                        args.insert(0, '_');
                    }
                    format!("{}${args}", fn_name(name))
                }
            };
            let mut name = base.clone();
            let mut k = 2;
            while !used.insert(name.clone()) {
                name = format!("{base}$${k}");
                k += 1;
            }
            name
        })
        .collect()
}

/// The Emela name of a function, without the type arguments of a monomorphized copy (for defect
/// messages, which name what the source wrote).
pub(crate) fn source_name(name: &str) -> &str {
    name.split_once('[').map_or(name, |(base, _)| base)
}

/// The JS name of a top-level function. A `$` is added when it clashes with a reserved word or a
/// global used by generated code. The `$` in names of suspending copies made by lowering
/// (`List.map$suspend1`) is kept: it never appears in Emela names, so it cannot clash with the
/// original.
pub(crate) fn fn_name(name: &str) -> String {
    let s: String = name.split('$').map(sanitize).collect::<Vec<_>>().join("$");
    if RESERVED.contains(&s.as_str()) {
        format!("{s}$")
    } else {
        s
    }
}

const RESERVED: &[&str] = &[
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "arguments",
    "eval",
    "undefined",
    "NaN",
    "Infinity",
    "Math",
    "BigInt",
    "Object",
    "Intl",
    "Symbol",
    "Number",
    "String",
    "Array",
    "Error",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monomorphized_names_are_readable_and_distinct() {
        let mut m = Module::new();
        let names = [
            "List.map",
            "List.map[Int, String, {}, {}]",
            "List.map$suspend8[Int, Int, {}, {suspends}]",
            "wrap[Option[Int]]",
            "wrap[Option[Int]]#2",
            "new",
            "new[{}]",
            "pair[(Int, String)]",
            "pair_Int[String]",
            "pair[Int_String]",
        ];
        for name in names {
            let f = m.declare(name, false);
            m.define(f, Vec::new(), TypeId::UNIT, Expr::Lit(Lit::Unit));
        }
        assert_eq!(
            function_names(&m),
            [
                "List_map",
                "List_map$Int_String",
                "List_map$suspend8$Int_Int_suspends",
                "wrap$Option_Int",
                "wrap$Option_Int_2",
                "new$",
                "new$$",
                "pair$Int_String",
                "pair_Int$String",
                // Clashes with `pair[(Int, String)]`.
                "pair$Int_String$$2",
            ]
        );
    }
}
