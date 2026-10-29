//! Renders LIR as readable text, for test snapshots and investigation.
//!
//! In the manner of the Core IR dump (`emela_core::dump`): `%n` for variables, with their
//! representation and Core types where defined (`x%3: i32 (Int)`), `'l%n` for labels, `[i32]` on
//! primitive operations, `!` after suspending calls and `on_fail 'l%n` / `on_fail fail` after
//! fallible ones. Each statement is on its own line, as `dst = value` or just `value` without a
//! destination; a sequence ends with its terminator, except that a `yield` without value is not
//! written. The keywords are those of the LIR design (`call`, `call_indirect`, `call_op`,
//! `builtin`, `make`, `field`, `tag`, `closure`, `ev_lookup`, `ev_extend`, `handler`, `fiber`,
//! `block`, `break`, `loop`, `recur`, `protect`, `cleanup`), plus `is_error`, `as_error`,
//! `runtime` and `coerce`, and those of the Heap IR (`make_closure f[captures]`, `env_get e#i`,
//! `env[types]` after the signature of a lifted function, and `on_suspend #state [saved]` after a
//! value with a suspension edge); the placeholder of `()` in a container is `()`. Functions are printed
//! in ID order.
//! The output is deterministic and not meant to be parsed back.
//!
//! ```text
//! export fn fact(n%0: i32 (Int)) -> i32 (Int) {
//!   r%3: i32 (Int) = loop(i%1: i32 (Int) = n%0, acc%2: i32 (Int) = 1) {
//!     z%4: i32 (Bool) = i%1 ==[i32] 0
//!     v%5: i32 (Int) = if z%4 {
//!       yield acc%2
//!     } else {
//!       j%6: i32 (Int) = i%1 -[i32] 1
//!       a%7: i32 (Int) = acc%2 *[i32] i%1
//!       recur(j%6, a%7)
//!     }
//!     yield v%5
//!   }
//!   return r%3
//! }
//! ```

use std::fmt::Write;

use emela_core::{CtorRef, HostModule, HostRef, Type, TypeId, VariantFields};

use crate::ir::{
    Callee, CaseKey, Const, Function, LabelId, Module, OnFail, Operand, SeqId, Stmt, SwitchOn,
    Term, Value, VarId,
};
use crate::repr::repr_of;

/// Every function, in ID order.
pub fn dump(m: &Module) -> String {
    let mut out = String::new();
    for (_, f) in m.functions.iter() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&dump_function(m, f));
    }
    out
}

/// One function. `f` need not be in the module yet; direct calls name the module's functions.
pub fn dump_function(m: &Module, f: &Function) -> String {
    let mut p = Printer {
        m,
        f,
        out: String::new(),
        indent: 0,
    };
    p.function();
    p.out
}

struct Printer<'a> {
    m: &'a Module,
    f: &'a Function,
    out: String,
    indent: usize,
}

impl Printer<'_> {
    fn function(&mut self) {
        let f = self.f;
        if f.exported {
            self.out.push_str("export ");
        }
        if f.suspends {
            self.out.push_str("suspend ");
        }
        if f.constant {
            self.out.push_str("const ");
        }
        if f.external.is_some() {
            self.out.push_str("external ");
        }
        let params: Vec<String> = f.params.iter().map(|&v| self.binder(v)).collect();
        write!(
            self.out,
            "fn {}({}) -> {} ({})",
            f.name,
            params.join(", "),
            repr_of(&self.m.core, f.ret),
            core_ty(&self.m.core, f.ret)
        )
        .unwrap();
        if f.fails {
            self.out.push_str(" fails");
        }
        if self.m.stage >= crate::ir::Stage::DefectsExplicit && f.defects {
            self.out.push_str(" defects");
        }
        if let Some(env) = &f.env {
            let tys: Vec<String> = env.iter().map(|&t| core_ty(&self.m.core, t)).collect();
            write!(self.out, " env[{}]", tys.join(", ")).unwrap();
        }
        if let Some(ext) = &f.external {
            let bindings: Vec<String> = ext
                .bindings
                .iter()
                .map(|b| format!("{}[{}]", b.target, host_ref(&b.host)))
                .collect();
            writeln!(self.out, " = {}", bindings.join(" ")).unwrap();
            return;
        }
        self.out.push(' ');
        match f.body {
            Some(body) => self.seq(body),
            None => self.out.push_str("<no body>"),
        }
        self.out.push('\n');
    }

    fn var(&self, v: VarId) -> String {
        format!("{}%{}", self.f.vars[v].hint, u32::from(v.into_raw()))
    }

    /// A variable where it is defined, with its types.
    fn binder(&self, v: VarId) -> String {
        let data = &self.f.vars[v];
        format!(
            "{}: {} ({})",
            self.var(v),
            data.repr,
            core_ty(&self.m.core, data.ty)
        )
    }

    fn label(&self, l: LabelId) -> String {
        format!("'{}%{}", self.f.labels[l].hint, u32::from(l.into_raw()))
    }

    fn operand(&self, o: &Operand) -> String {
        match o {
            Operand::Var(v) => self.var(*v),
            Operand::Const(c) => const_text(c),
        }
    }

    fn operands(&self, os: &[Operand]) -> String {
        os.iter()
            .map(|o| self.operand(o))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn vars(&self, vs: &[VarId]) -> String {
        vs.iter()
            .map(|&v| self.var(v))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn ctor_name(&self, c: CtorRef) -> String {
        format!(
            "{}.{}",
            self.m.core.enums[c.enum_id].name,
            self.m.core.variant(c).name
        )
    }

    fn effect_name(&self, e: emela_core::EffectId) -> &str {
        &self.m.core.effects[e].name
    }

    fn newline(&mut self) {
        self.out.push('\n');
        for _ in 0..self.indent {
            self.out.push_str("  ");
        }
    }

    /// A sequence in `{` and `}`.
    fn seq(&mut self, id: SeqId) {
        let seq = &self.f.seqs[id];
        if seq.stmts.is_empty() && seq.term == Term::Yield(None) {
            self.out.push_str("{}");
            return;
        }
        self.out.push('{');
        self.indent += 1;
        for &s in &seq.stmts {
            self.newline();
            self.stmt(&self.f.stmts[s]);
        }
        if seq.term != Term::Yield(None) {
            self.newline();
            self.term(&seq.term);
        }
        self.indent -= 1;
        self.newline();
        self.out.push('}');
    }

    fn term(&mut self, t: &Term) {
        let with_value = |kw: &str, v: &Option<Operand>| match v {
            Some(v) => format!("{kw} {}", self.operand(v)),
            None => kw.to_owned(),
        };
        let text = match t {
            Term::Yield(v) => with_value("yield", v),
            Term::Return(v) => with_value("return", v),
            Term::Fail(v) => format!("fail {}", self.operand(v)),
            Term::Break { label, value } => match value {
                Some(v) => format!("break {}({})", self.label(*label), self.operand(v)),
                None => format!("break {}", self.label(*label)),
            },
            Term::Recur(args) => format!("recur({})", self.operands(args)),
            Term::Defect(msg, None) => format!("defect({})", self.operand(msg)),
            Term::Defect(msg, Some(at)) => format!("defect({}) at {at:?}", self.operand(msg)),
            Term::Unwind => "unwind".to_owned(),
            Term::Unreachable => "unreachable".to_owned(),
        };
        self.out.push_str(&text);
    }

    fn stmt(&mut self, s: &Stmt) {
        if let Some(dst) = s.dst() {
            write!(self.out, "{} = ", self.binder(dst)).unwrap();
        }
        match s {
            Stmt::Let {
                value,
                on_defect,
                on_suspend,
                ..
            } => {
                self.value(value);
                if let Some(edge) = on_suspend {
                    write!(
                        self.out,
                        " on_suspend #{} [{}]",
                        edge.state,
                        self.vars(&edge.saved)
                    )
                    .unwrap();
                }
                if let Value::Call {
                    on_fail: Some(OnFail::Handle { body, .. }),
                    ..
                }
                | Value::Fiber {
                    on_fail: Some(OnFail::Handle { body, .. }),
                    ..
                } = value
                {
                    self.out.push(' ');
                    self.landing(*body);
                }
                if let Some(l) = on_defect {
                    self.out.push_str(" on_defect ");
                    self.landing(*l);
                }
            }
            Stmt::If {
                cond, then, else_, ..
            } => {
                write!(self.out, "if {} ", self.operand(cond)).unwrap();
                self.seq(*then);
                self.out.push_str(" else ");
                self.seq(*else_);
            }
            Stmt::Switch {
                on, cases, default, ..
            } => {
                match on {
                    SwitchOn::Tag(v) => write!(self.out, "switch tag({}) {{", self.var(*v)),
                    SwitchOn::Lit(v) => write!(self.out, "switch {} {{", self.var(*v)),
                }
                .unwrap();
                self.indent += 1;
                for case in cases {
                    self.newline();
                    let key = match (&case.key, on) {
                        (CaseKey::Variant(i), SwitchOn::Tag(v)) => {
                            match self.m.core.ty(self.f.vars[*v].ty) {
                                Type::Enum(id, _) => self.ctor_name(CtorRef {
                                    enum_id: *id,
                                    variant: *i,
                                }),
                                _ => format!("#{i}"),
                            }
                        }
                        (CaseKey::Variant(i), SwitchOn::Lit(_)) => format!("#{i}"),
                        (CaseKey::Lit(c), _) => const_text(c),
                    };
                    write!(self.out, "case {key} ").unwrap();
                    self.seq(case.body);
                }
                if let Some(d) = default {
                    self.newline();
                    self.out.push_str("default ");
                    self.seq(*d);
                }
                self.indent -= 1;
                self.newline();
                self.out.push('}');
            }
            Stmt::Loop {
                vars, inits, body, ..
            } => {
                let bindings: Vec<String> = vars
                    .iter()
                    .zip(inits)
                    .map(|(&v, i)| format!("{} = {}", self.binder(v), self.operand(i)))
                    .collect();
                write!(self.out, "loop({}) ", bindings.join(", ")).unwrap();
                self.seq(*body);
            }
            Stmt::Block { label, body, .. } => {
                write!(self.out, "block {} ", self.label(*label)).unwrap();
                self.seq(*body);
            }
            Stmt::Dup(v) => write!(self.out, "dup {}", self.var(*v)).unwrap(),
            Stmt::Drop(v) => write!(self.out, "drop {}", self.var(*v)).unwrap(),
            Stmt::Protect { body, cleanup, .. } => {
                self.out.push_str("protect ");
                self.seq(*body);
                self.out.push_str(" cleanup ");
                self.seq(*cleanup);
            }
        }
    }

    fn on_fail(&self, on_fail: &Option<OnFail>) -> String {
        match on_fail {
            None => String::new(),
            Some(OnFail::Break(l)) => format!(" on_fail {}", self.label(*l)),
            Some(OnFail::Propagate) => " on_fail fail".to_owned(),
            Some(OnFail::Handle { error, .. }) => format!(" on_fail({})", self.binder(*error)),
        }
    }

    /// A landing (`on_fail(e) { … }`, `on_defect { … }`): on one line if it is just its
    /// terminator.
    fn landing(&mut self, id: SeqId) {
        let seq = &self.f.seqs[id];
        if seq.stmts.is_empty() {
            self.out.push_str("{ ");
            self.term(&seq.term);
            self.out.push_str(" }");
        } else {
            self.seq(id);
        }
    }

    fn value(&mut self, v: &Value) {
        let text = match v {
            Value::Const(c) => const_text(c),
            Value::Prim { op, ty, args } => match args.as_slice() {
                [a] => format!("{}[{}] {}", op.symbol(), ty.name(), self.operand(a)),
                [a, b] => format!(
                    "{} {}[{}] {}",
                    self.operand(a),
                    op.symbol(),
                    ty.name(),
                    self.operand(b)
                ),
                _ => format!("{}[{}]({})", op.symbol(), ty.name(), self.operands(args)),
            },
            Value::Call {
                callee,
                args,
                suspends,
                on_fail,
            } => {
                let callee = match callee {
                    Callee::Direct(f) => format!("call {}", self.m.functions[*f].name),
                    Callee::Indirect(c) => format!("call_indirect {}", self.var(*c)),
                    Callee::Op {
                        handler,
                        effect,
                        op,
                    } => {
                        let def = &self.m.core.effects[*effect];
                        let op = def.ops.get(*op).map_or("?", |o| o.name.as_str());
                        format!("call_op {}.{}#{op}", self.var(*handler), def.name)
                    }
                };
                format!(
                    "{callee}({}){}{}",
                    self.operands(args),
                    if *suspends { "!" } else { "" },
                    self.on_fail(on_fail)
                )
            }
            Value::Builtin { op, args } => {
                format!("builtin {}({})", op.info().path(), self.operands(args))
            }
            Value::Make { ctor, args } if args.is_empty() => {
                format!("make {}", self.ctor_name(*ctor))
            }
            Value::Make { ctor, args } => {
                format!("make {}({})", self.ctor_name(*ctor), self.operands(args))
            }
            Value::Field { base, ctor, index } => {
                // The index is what the IR holds; the name is added for readability if present.
                let name = match &self.m.core.variant(*ctor).fields {
                    VariantFields::Named(names) if *index < names.len() => {
                        format!(".{}", names[*index])
                    }
                    _ => String::new(),
                };
                format!(
                    "field {} {}{name}#{index}",
                    self.var(*base),
                    self.ctor_name(*ctor)
                )
            }
            Value::Tag(v) => format!("tag {}", self.var(*v)),
            Value::Tuple(es) => format!("tuple({})", self.operands(es)),
            Value::TupleGet { base, index } => format!("tuple_get {}#{index}", self.var(*base)),
            Value::List { elems, tail } => {
                let mut parts: Vec<String> = elems.iter().map(|e| self.operand(e)).collect();
                if let Some(t) = tail {
                    parts.push(format!("..{}", self.operand(t)));
                }
                format!("list[{}]", parts.join(", "))
            }
            Value::ListIsEmpty(v) => format!("list_is_empty {}", self.var(*v)),
            Value::ListHead(v) => format!("list_head {}", self.var(*v)),
            Value::ListTail(v) => format!("list_tail {}", self.var(*v)),
            Value::FnRef(f) => format!("fn_ref {}", self.m.functions[*f].name),
            Value::Closure(c) => {
                let params: Vec<String> = c.params.iter().map(|&v| self.binder(v)).collect();
                write!(
                    self.out,
                    "{}closure({}) [{}] ",
                    if c.suspends { "suspend " } else { "" },
                    params.join(", "),
                    self.vars(&c.captures)
                )
                .unwrap();
                self.seq(c.body);
                return;
            }
            Value::MakeClosure { function, captures } => format!(
                "make_closure {}[{}]",
                self.m.functions[*function].name,
                self.vars(captures)
            ),
            Value::EnvGet { env, index } => format!("env_get {}#{index}", self.var(*env)),
            Value::EvLookup { ev, effect } => {
                format!(
                    "ev_lookup({}, {})",
                    self.var(*ev),
                    self.effect_name(*effect)
                )
            }
            Value::EvExtend { base, bindings } => {
                let bindings: Vec<String> = bindings
                    .iter()
                    .map(|(e, h)| format!("{}: {}", self.effect_name(*e), self.var(*h)))
                    .collect();
                match base {
                    Some(b) => format!("ev_extend({}, [{}])", self.var(*b), bindings.join(", ")),
                    None => format!("ev_extend([{}])", bindings.join(", ")),
                }
            }
            Value::Handler { name, effect, ops } => {
                let def = &self.m.core.effects[*effect];
                let ops: Vec<String> = ops
                    .iter()
                    .enumerate()
                    .map(|(i, &h)| {
                        let op = def.ops.get(i).map_or("?", |o| o.name.as_str());
                        format!("{op}: {}", self.var(h))
                    })
                    .collect();
                format!("handler {name}: {} {{ {} }}", def.name, ops.join(", "))
            }
            Value::Fiber { op, args, on_fail } => format!(
                "fiber {}({}){}{}",
                op.name(),
                self.operands(args),
                if op.suspends() { "!" } else { "" },
                self.on_fail(on_fail)
            ),
            Value::IsError { error, enum_id } => format!(
                "is_error {} {}",
                self.var(*error),
                self.m.core.enums[*enum_id].name
            ),
            Value::AsError { error, enum_id } => format!(
                "as_error {} {}",
                self.var(*error),
                self.m.core.enums[*enum_id].name
            ),
            Value::Runtime { op, args } => {
                format!("runtime {}({})", op.name(), self.operands(args))
            }
            Value::Coerce(v) => format!("coerce {}", self.var(*v)),
        };
        self.out.push_str(&text);
    }
}

/// A Core type, written as the Core dump writes it (a copy: the Core dump's own function is
/// private).
pub fn core_ty(core: &emela_core::Module, ty: TypeId) -> String {
    let list = |ts: &[TypeId]| {
        ts.iter()
            .map(|&t| core_ty(core, t))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match core.ty(ty) {
        Type::Int => "Int".into(),
        Type::Int64 => "Int64".into(),
        Type::Float => "Float".into(),
        Type::Bool => "Bool".into(),
        Type::String => "String".into(),
        Type::Bytes => "Bytes".into(),
        Type::Unit => "()".into(),
        Type::Never => "Never".into(),
        Type::Tuple(ts) => format!("({})", list(ts)),
        Type::List(t) => format!("List[{}]", core_ty(core, *t)),
        Type::Enum(id, ts) if ts.is_empty() => core.enums[*id].name.clone(),
        Type::Enum(id, ts) => format!("{}[{}]", core.enums[*id].name, list(ts)),
        Type::Param(i) => char::from(b'A' + *i as u8).to_string(),
        Type::Fn(f) => {
            let mut s = format!("fn({}) -> {}", list(&f.params), core_ty(core, f.ret));
            for (flag, name) in [
                (f.fails, " fails"),
                (f.suspends, " suspends"),
                (f.ev, " ev"),
            ] {
                if flag {
                    s.push_str(name);
                }
            }
            s
        }
        Type::Handler(e) => format!("handler {}", core.effects[*e].name),
        Type::Evidence => "evidence".into(),
        Type::Cell(t) => format!("cell {}", core_ty(core, *t)),
        Type::FiberScope => "fiber_scope".into(),
        Type::Any => "any".into(),
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
            parts.extend(set.params.iter().map(|&p| core_ty(core, p)));
            format!("{{{}}}", parts.join(", "))
        }
    }
}

/// Int64 gets an `i64` suffix, and Float always has a decimal point or exponent.
fn const_text(c: &Const) -> String {
    match c {
        Const::Int(n) => n.to_string(),
        Const::Int64(n) => format!("{n}i64"),
        Const::Float(x) => format!("{x:?}"),
        Const::Bool(b) => b.to_string(),
        Const::String(s) => format!("{s:?}"),
        Const::Unit => "()".to_owned(),
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
