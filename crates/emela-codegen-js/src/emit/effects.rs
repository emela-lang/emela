//! Emits effects (spec chapter 8) and external functions (spec chapter 12).
//!
//! Handlers are passed as arguments and called directly (spec 16.5). The evidence table is an
//! object mapping effect names to handler values, passed as the last argument of functions and
//! lambdas whose R is not empty. `with` creates a new object extending the table via `__proto__`
//! (the outer table is left unchanged). A handler value is an object mapping operation names to
//! functions.
//!
//! An external function becomes a wrapper that calls the host function. Thrown exceptions become
//! defects (spec 12.5), and lone surrogates in strings received from the host are replaced with
//! U+FFFD (spec 12.5). The wrapper for an `@external suspend fn` is a generator that awaits the
//! Promise the host returns, passing an AbortSignal as the last argument (spec 12.2).

use emela_core::{
    EffectId, Expr, External, FnId, Function, HostModule, HostRef, Module, Release, Type, TypeId,
    WithBind,
};

use super::{
    Dest, Emitter, function_keyword, is_identifier_name, js_string, sanitize, source_name,
};

/// Whether evaluating `e` may suspend (a suspending call or fiber operation outside lambdas).
fn may_suspend(e: &Expr) -> bool {
    match e {
        Expr::Lambda { .. } => false,
        Expr::Call { suspends: true, .. } => true,
        Expr::Fiber { op, .. } if op.suspends() => true,
        _ => {
            let mut found = false;
            e.for_each_child(&mut |c| found = found || may_suspend(c));
            found
        }
    }
}

/// The evidence table key. For an effect with type arguments (`State.State[Int]`, spec 8.1),
/// turning the name into an identifier could collapse the type argument delimiters and collide,
/// so the IR effect index is appended to keep keys distinct.
///
/// Keys start with an uppercase letter (effect names do), so they never shadow a name on
/// `Object.prototype` that the table would otherwise inherit.
pub(crate) fn effect_key(m: &Module, effect: EffectId) -> String {
    let name = &m.effects[effect].name;
    if name.contains('[') {
        format!("{}${}", sanitize(name), effect.into_raw().into_u32())
    } else {
        sanitize(name)
    }
}

/// Accessing an operation on a handler value (`.print_line`).
pub(crate) fn op_access(name: &str) -> String {
    if name == "__proto__" {
        "[\"__proto__\"]".to_owned()
    } else {
        format!(".{name}")
    }
}

/// A key in a handler value's object literal.
fn op_key(name: &str) -> String {
    if name == "__proto__" {
        "[\"__proto__\"]".to_owned()
    } else {
        name.to_owned()
    }
}

/// An import specifier. Absolute paths (rewritten by lowering to files next to the source) become
/// `file:` URLs; anything else (`node:fs`, package names) is kept as written.
pub(crate) fn module_specifier(module: &str) -> String {
    if !module.starts_with('/') {
        return module.to_owned();
    }
    let mut url = String::from("file://");
    for b in module.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            url.push(b as char);
        } else {
            url.push_str(&format!("%{b:02X}"));
        }
    }
    url
}

impl Emitter<'_> {
    /// `{ __proto__: base, Console: h }`．
    ///
    /// Each `with` makes a new table chained to the outer one instead of writing into it. A
    /// capability taken out of the outer table, or a closure that captured the outer table, keeps
    /// seeing the outer handlers.
    pub(super) fn provide(&mut self, base: Option<&Expr>, bindings: &[(EffectId, Expr)]) -> String {
        let mut es: Vec<&Expr> = base.into_iter().collect();
        es.extend(bindings.iter().map(|(_, h)| h));
        let mut vs = self.values_in_order(&es);
        let base = match base {
            Some(_) => vs.remove(0),
            None => "null".to_owned(),
        };
        let mut s = format!("{{ __proto__: {base}");
        for ((effect, _), v) in bindings.iter().zip(vs) {
            s.push_str(&format!(", {}: {v}", effect_key(self.m, *effect)));
        }
        s.push_str(" }");
        s
    }

    /// `{ print_line: (text) => …, … }`. Suspend fn operations are tagged with the handler and
    /// operation names via `$suspendOp`, for the defect message of a second resume (spec 14.5).
    pub(super) fn handler(&mut self, handler: &str, effect: EffectId, ops: &[Expr]) -> String {
        let m = self.m;
        let names = &m.effects[effect].ops;
        assert_eq!(
            names.len(),
            ops.len(),
            "invalid IR: wrong number of handler operations"
        );
        let es: Vec<&Expr> = ops.iter().collect();
        let vs = self.values_in_order(&es);
        let fields: Vec<String> = names
            .iter()
            .zip(vs)
            .map(|(op, v)| {
                let v = if op.suspend {
                    format!(
                        "{}({}, {}, {v})",
                        self.rt("$suspendOp"),
                        js_string(handler),
                        js_string(&op.name)
                    )
                } else {
                    v
                };
                format!("{}: {v}", op_key(&op.name))
            })
            .collect();
        format!("{{ {} }}", fields.join(", "))
    }

    /// Bindings of a `with` without release (handler values and evidence tables).
    pub(super) fn with_binds(&mut self, binds: &[WithBind]) {
        for b in binds {
            assert!(
                b.release.is_none(),
                "bindings with release are emitted by with_"
            );
            self.bind(b);
        }
    }

    fn bind(&mut self, b: &WithBind) -> String {
        let v = self.value(&b.value);
        let name = self.local(b.var);
        self.line(&format!("const {name} = {v};"));
        name
    }

    /// Statements that pass the value of a `with` to `dest` (spec 14.2, 14.4).
    ///
    /// For each binding with release, the later bindings and the body are wrapped in `try`, and
    /// release is evaluated in `finally`. A failure of a later binding (init) or of the body leaves
    /// the `try` by `return` or `break`, so releases run in reverse binding order. When unwinding
    /// while suspended, the runtime throws `$Interrupted` into the waiting continuation, which
    /// runs the same `finally` from the inside out. Throwing replaced closing the generator with
    /// `gen.return()` so that interruption works per fiber: a scope (`$scope`) can catch the
    /// interruption aimed at its body on the way.
    ///
    /// ```js
    /// const h$1 = Pool();
    /// let $t0;                         // what the body threw (a defect or $Interrupted)
    /// try {
    ///   …                              // later bindings and the body
    /// } catch ($e) {
    ///   $t0 = $e;
    ///   throw $e;
    /// } finally {
    ///   const $t1 = $hold();           // only when the release may suspend (spec 11.4)
    ///   try {
    ///     …                            // release
    ///   } catch ($e) {
    ///     throw $releaseDefect($t0, $e, "Pool");
    ///   } finally {
    ///     $retire(h$1, "Pool");
    ///     $unhold($t1);
    ///   }
    /// }
    /// ```
    pub(super) fn with_(&mut self, binds: &[WithBind], body: &Expr, dest: &Dest) {
        let mut open: Vec<(String, String, &Release)> = Vec::new();
        for b in binds {
            let name = self.bind(b);
            if let Some(r) = &b.release {
                let thrown = self.tmp();
                self.line(&format!("let {thrown};"));
                self.line("try {");
                self.indent += 1;
                open.push((name, thrown, r));
            }
        }
        self.stmt(body, dest);
        for (name, thrown, r) in open.into_iter().rev() {
            self.indent -= 1;
            self.line("} catch ($e) {");
            self.line(&format!("  {thrown} = $e;"));
            self.line("  throw $e;");
            self.line("} finally {");
            self.indent += 1;
            // A release that may suspend is not interrupted until it ends (spec 11.4).
            let hold = may_suspend(&r.body).then(|| {
                let h = self.tmp();
                let f = self.rt("$hold");
                self.line(&format!("const {h} = {f}();"));
                h
            });
            self.line("try {");
            self.indent += 1;
            // The release value is discarded. It cannot fail (E is empty, spec 8.4), so it needs
            // no target.
            let v = self.value(&r.body);
            if !super::is_atom(&r.body) {
                self.line(&format!("{v};"));
            }
            self.indent -= 1;
            let handler = js_string(&r.handler);
            let merge = self.rt("$releaseDefect");
            self.line("} catch ($e) {");
            self.line(&format!("  throw {merge}({thrown}, $e, {handler});"));
            let retire = self.rt("$retire");
            self.line("} finally {");
            self.line(&format!("  {retire}({name}, {handler});"));
            if let Some(h) = hold {
                let f = self.rt("$unhold");
                self.line(&format!("  {f}({h});"));
            }
            self.line("}");
            self.indent -= 1;
            self.line("}");
        }
    }

    /// The wrapper of an external function. Without a JS `@external`, calling it is a defect
    /// (lowering already reports functions reachable from the entry, so only unreachable ones get
    /// here).
    ///
    /// The wrapper of an `@external suspend fn` (`Function::suspends`) is a generator: it awaits
    /// the host function's return value (a Promise) with `yield $await(…)` and converts the
    /// resolved value by the same rules as a return value. On rejection, the driver turns the
    /// reason into a defect with `$foreign` and throws it at the waiting point (spec 12.5).
    pub(super) fn external_fn(&mut self, id: FnId, f: &Function, ext: &External) {
        let params: Vec<String> = f.params.iter().map(|&p| self.local(p)).collect();
        let param_tys: Vec<TypeId> = f.params.iter().map(|&p| self.m.locals[p].ty).collect();
        self.line(&format!(
            "{} {}({}) {{",
            function_keyword(f.suspends),
            self.fn_js(id),
            params.join(", ")
        ));
        self.indent += 1;
        let callee = ext.binding("js").map(|b| match &b.host {
            HostRef::Global(path) => global_ref(path),
            HostRef::Import { module, name } => {
                let specifier = match module {
                    HostModule::Named(m) | HostModule::File(m) => m.clone(),
                    HostModule::Bundled(path) => format!("./{}/{path}", crate::BUNDLED_DIR),
                };
                self.import(&specifier, name)
            }
        });
        match callee {
            Some(callee) => {
                let args: Vec<String> = params
                    .iter()
                    .zip(&param_tys)
                    .map(|(p, &ty)| self.convert(ty, p, Dir::ToHost, 0))
                    .collect();
                let foreign = self.rt("$foreign");
                if f.suspends {
                    self.suspending_external(&callee, &args, f.ret, foreign);
                    self.indent -= 1;
                    self.line("}");
                    return;
                }
                let call = format!("{callee}({})", args.join(", "));
                self.line("try {");
                self.indent += 1;
                match f.ret {
                    TypeId::UNIT => {
                        self.line(&format!("{call};"));
                        self.line("return undefined;");
                    }
                    ret => {
                        let v = self.convert(ret, &call, Dir::FromHost, 0);
                        self.line(&format!("return {v};"));
                    }
                }
                self.indent -= 1;
                self.line("} catch ($e) {");
                self.line(&format!("  throw {foreign}($e);"));
                self.line("}");
            }
            None => {
                let no = self.rt("$noExternal");
                self.line(&format!(
                    "return {no}({});",
                    js_string(source_name(&f.name))
                ));
            }
        }
        self.indent -= 1;
        self.line("}");
    }

    /// Calls the host function `callee` with `args` (converted JS expressions) and an AbortSignal
    /// as the last argument (spec 12.2). The runtime aborts it when the waiting fiber is
    /// interrupted.
    fn suspending_external(&mut self, callee: &str, args: &[String], ret: TypeId, foreign: &str) {
        let abortable = self.rt("$abortable");
        self.line(&format!("const $c = {abortable}();"));
        let mut args = args.to_vec();
        args.push("$c.signal".to_owned());
        self.line("let $p;");
        self.line("try {");
        self.line(&format!("  $p = {callee}({});", args.join(", ")));
        self.line("} catch ($e) {");
        self.line(&format!("  throw {foreign}($e);"));
        self.line("}");
        let wait = format!("(yield {}($p, $c))", self.rt("$await"));
        match ret {
            TypeId::UNIT => {
                self.line(&format!("{wait};"));
                self.line("return undefined;");
            }
            ret => {
                self.line(&format!("const $v = {wait};"));
                let v = self.convert(ret, "$v", Dir::FromHost, 0);
                self.line(&format!("return {v};"));
            }
        }
    }

    /// Imports `name` from the module and returns its local name. Each is imported only once.
    fn import(&mut self, module: &str, name: &str) -> String {
        if let Some((_, _, local)) = self
            .imports
            .iter()
            .find(|(m, n, _)| m == module && n == name)
        {
            return local.clone();
        }
        let local = format!("$ext{}", self.imports.len());
        self.imports
            .push((module.to_owned(), name.to_owned(), local.clone()));
        local
    }

    /// The JS expression converting `v` (a JS expression, used once) of type `ty` across the
    /// boundary in direction `dir` (spec 12.4, 12.5). Strings from the host are made well formed
    /// and callbacks are wrapped to convert their arguments and results; other values pass as is.
    fn convert(&mut self, ty: TypeId, v: &str, dir: Dir, depth: usize) -> String {
        let m = self.m;
        match (m.ty(ty), dir) {
            (Type::String, Dir::FromHost) => format!("{}({v})", self.rt("$wellFormed")),
            (Type::Fn(f), _) => {
                let (params, ret) = (&f.params, f.ret);
                // Callback arguments convert the opposite way; return values the same way.
                let names: Vec<String> = (0..params.len())
                    .map(|i| format!("$a{depth}_{i}"))
                    .collect();
                let args: Vec<String> = params
                    .iter()
                    .zip(&names)
                    .map(|(&p, n)| self.convert(p, n, dir.flip(), depth + 1))
                    .collect();
                let call = format!("$f{depth}({})", args.join(", "));
                let body = self.convert(ret, &call, dir, depth + 1);
                format!("(($f{depth}) => ({}) => {body})({v})", names.join(", "))
            }
            _ => v.to_owned(),
        }
    }
}

#[derive(Clone, Copy)]
enum Dir {
    ToHost,
    FromHost,
}

impl Dir {
    fn flip(self) -> Dir {
        match self {
            Dir::ToHost => Dir::FromHost,
            Dir::FromHost => Dir::ToHost,
        }
    }
}

/// A global reference (`console.log`). Non-identifier segments become `globalThis["…"]`
/// subscripts.
fn global_ref(path: &str) -> String {
    let parts: Vec<&str> = path.split('.').collect();
    if parts.iter().all(|p| is_identifier_name(p)) {
        return path.to_owned();
    }
    let mut s = String::from("globalThis");
    for p in parts {
        s.push_str(&format!("[{}]", js_string(p)));
    }
    s
}
