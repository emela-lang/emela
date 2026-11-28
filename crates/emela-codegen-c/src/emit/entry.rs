//! What runs the program: the cleanup of the constants, the C exports and the generated `main`.

use super::external::HostTy;
use super::*;

impl Emitter<'_> {
    pub(super) fn cleanup(&self, out: &mut Writer) {
        out.static_func("void em_cleanup(void)", |w| {
            for &c in &self.consts {
                w.stmt(&format!("em_const_release(&em_c{})", ix(c)));
            }
        });
        out.blank();
    }

    /// The function the run calls for the entry `id` (main or a test function, which take no
    /// arguments): it calls it and returns its value as a word. `em_run` runs it as the root
    /// fiber (spec 14.5) and drops the value. A value held by value is boxed: `em_run` knows a
    /// word and its representation, which a struct is not.
    pub(super) fn entry_thunk(&mut self, out: &mut Writer, id: FnId) -> (String, CTy) {
        let f = &self.m.functions[id];
        let name = self.fn_names[ix(id)].clone();
        if !f.params.is_empty() {
            self.internal(format!("the entry `{}` takes parameters", f.name));
        }
        let ret = self.ct(f.ret);
        let (c_ty, boxed) = match ret {
            CTy::Agg(i) => {
                let c_ty = self.cty(ret).to_owned();
                let boxed = self.box_of("v", i);
                (c_ty, Some(boxed))
            }
            CTy::Scalar(_) => (String::new(), None),
        };
        let thunk = format!("{name}_entry");
        let no_args: [&str; 0] = [];
        out.static_func(&signature("em_word", &thunk, no_args), |w| {
            w.decl("em_word", "w", None);
            w.assign("w.l", "0");
            let entry = call(&name, no_args);
            match ret {
                CTy::Scalar(Repr::Void) => w.stmt(&entry),
                CTy::Scalar(r) => w.assign(&format!("w.{}", acc(r)), &entry),
                CTy::Agg(_) => {
                    w.decl(&c_ty, "v", Some(&entry));
                    w.assign("w.p", boxed.as_deref().unwrap_or("NULL"));
                }
            }
            w.ret(Some("w"));
        });
        out.blank();
        (thunk, ret)
    }

    /// The C functions of the exported fns (`[target.native.export]`): for each host name, a function
    /// with external linkage and the prototype of the table in `[ext.boundary]`. It stores its
    /// arguments in statics and runs a thunk as one launch; the thunk converts them, calls the fn
    /// and stores the converted value. A fn with a callback is not supported yet.
    pub(super) fn c_exports(&mut self, out: &mut Writer, exports: &[(FnId, Vec<String>)]) {
        for (id, names) in exports {
            let f = &self.m.functions[*id];
            let fname = self.fn_names[ix(*id)].clone();
            let shown = display_name(&f.name).to_owned();
            let mut params = Vec::new();
            let mut ok = true;
            for &p in &f.params {
                match self.host_ty(f.vars[p].ty) {
                    Some(t) => params.push((p, t)),
                    None => {
                        self.export_type_error(&shown, f.vars[p].ty, "takes");
                        ok = false;
                    }
                }
            }
            let ret_ty = if f.ret == TypeId::UNIT || f.ret == TypeId::NEVER {
                Some(HostTy::Unit)
            } else {
                self.host_ty(f.ret)
            };
            let Some(ret_ty) = ret_ty else {
                self.export_type_error(&shown, f.ret, "returns");
                continue;
            };
            if !ok {
                continue;
            }
            let arg = |i: usize| format!("{fname}_xa{i}");
            let result = format!("{fname}_xr");
            for (i, (_, t)) in params.iter().enumerate() {
                if *t != HostTy::Unit {
                    for (ty, suffix) in t.params().iter().zip(t.suffixes()) {
                        self.statics.decl(
                            &format!("static {ty}"),
                            &format!("{}{suffix}", arg(i)),
                            None,
                        );
                    }
                }
            }
            if ret_ty != HostTy::Unit {
                self.statics
                    .decl(&format!("static {}", ret_ty.ret()), &result, None);
            }
            let thunk = format!("{fname}_xthunk");
            let name_lit = names::string_literal(shown.as_bytes());
            let ret = CTy::Scalar(self.repr(f.ret));
            let ret_c = cty(ret.repr());
            let no_args: [&str; 0] = [];
            out.static_func(&signature("em_word", &thunk, no_args), |w| {
                w.decl("em_word", "w", None);
                w.assign("w.l", "0");
                let mut args = Vec::new();
                let mut strings: Vec<String> = Vec::new();
                for (i, (p, t)) in params.iter().enumerate() {
                    let a = arg(i);
                    args.push(match t {
                        HostTy::String | HostTy::Bytes => {
                            let s = format!("s{i}");
                            let value = if *t == HostTy::String {
                                call("em_from_export_string", [a.as_str(), name_lit.as_str()])
                            } else {
                                call(
                                    "em_from_export_bytes",
                                    [format!("{a}_data"), format!("{a}_len"), name_lit.clone()],
                                )
                            };
                            w.decl("em_ref", &s, Some(&value));
                            w.if_begin(&format!("{s} == NULL"));
                            for earlier in &strings {
                                w.stmt(&call("em_drop", [earlier.as_str()]));
                            }
                            w.ret(Some("w"));
                            w.end();
                            strings.push(s.clone());
                            s
                        }
                        HostTy::Bool => format!("({a} ? 1 : 0)"),
                        HostTy::Handle => call("em_host_box", [&a]),
                        HostTy::Unit => dummy(
                            CTy::Scalar(self.m.functions[*id].vars[*p].repr),
                            &self.values,
                        )
                        .unwrap_or_else(|| "0".to_owned()),
                        _ => a,
                    });
                }
                let value = call(&fname, &args);
                if ret.is_void() {
                    w.stmt(&value);
                    return w.ret(Some("w"));
                }
                w.decl(ret_c, "r", Some(&value));
                w.if_return("em_defect != NULL", Some("w"));
                match ret_ty {
                    HostTy::Unit => {}
                    HostTy::String => w.assign(&result, &call("em_to_export_string", ["r"])),
                    HostTy::Bytes => w.assign(&result, &call("em_to_export_bytes", ["r"])),
                    HostTy::Bool => w.assign(&result, "(r != 0)"),
                    HostTy::Handle => {
                        w.assign(&result, &call("em_host_get", ["r"]));
                        w.stmt(&call("em_drop", ["r"]));
                    }
                    _ => w.assign(&result, "r"),
                }
                w.ret(Some("w"));
            });
            out.blank();
            for name in names {
                if !names::is_c_identifier(name) {
                    self.errors.push(CError::new(
                        CErrorKind::Unsupported,
                        format!(
                            "`{name}` in `@export(c, ...)` of `{shown}` is not a C function name"
                        ),
                    ));
                    continue;
                }
                let c_params: Vec<String> = params
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, t))| *t != HostTy::Unit)
                    .flat_map(|(i, (_, t))| {
                        t.params()
                            .iter()
                            .zip(t.suffixes())
                            .map(move |(ty, suffix)| param(ty, &format!("a{i}{suffix}")))
                    })
                    .collect();
                let c_params = if c_params.is_empty() {
                    vec!["void".to_owned()]
                } else {
                    c_params
                };
                out.func(&signature(ret_ty.ret(), name, c_params), |w| {
                    w.stmt(&call("em_export_enter", [name_lit.as_str()]));
                    for (i, (_, t)) in params.iter().enumerate() {
                        if *t != HostTy::Unit {
                            for suffix in t.suffixes() {
                                w.assign(&format!("{}{suffix}", arg(i)), &format!("a{i}{suffix}"));
                            }
                        }
                    }
                    w.stmt(&call("em_run", [thunk.as_str(), "EM_REPR_VOID"]));
                    w.stmt("em_export_leave()");
                    if ret_ty != HostTy::Unit {
                        w.ret(Some(&result));
                    }
                });
                out.blank();
            }
        }
    }

    /// `verb` is `takes` or `returns`.
    pub(super) fn export_type_error(&mut self, function: &str, ty: TypeId, verb: &str) {
        // Arrays are E5204 for exported fns too (`[ext.boundary]`).
        if matches!(self.core().ty(ty), Type::Array(_)) {
            let name = emela_lir::core_ty(self.core(), ty);
            self.errors.push(CError::new(
                CErrorKind::BoundaryType,
                format!(
                    "exported function `{function}` {verb} `{name}`, which target `native` does not support yet"
                ),
            ));
            return;
        }
        // Not E5201, which is about external functions: an exported fn with a callback has no
        // code of its own yet (the form of callbacks in C is open, 18.1 #38).
        let what = if matches!(self.core().ty(ty), Type::Fn(_)) {
            format!(
                "exported function `{function}` takes a callback, which target `native` does not support yet"
            )
        } else {
            let name = emela_lir::core_ty(self.core(), ty);
            format!("exported function `{function}` uses the type {name}, which cannot cross to C")
        };
        self.errors.push(CError::new(CErrorKind::Unsupported, what));
    }

    /// `int main(int argc, char **argv)`: starts the runtime, runs `body` and exits. `start` is
    /// the call that starts the runtime.
    pub(super) fn main_func(out: &mut Writer, start: &str, body: impl FnOnce(&mut Writer)) {
        let sig = signature("int", "main", ["int argc", "char **argv"]);
        out.func(&sig, |w| {
            w.stmt(start);
            body(w);
            w.ret(Some("em_exit(em_cleanup)"));
        });
    }

    /// Runs the tests one by one and reports each result (the JS test runner's protocol).
    pub(super) fn c_test_main(&mut self, out: &mut Writer, tests: &[(String, FnId)]) {
        self.cleanup(out);
        let thunks: Vec<(String, CTy)> = tests
            .iter()
            .map(|(_, id)| self.entry_thunk(out, *id))
            .collect();
        // The arguments that the test runner passes are not the program's: `Args` returns `[]`
        // under `emela test` (`[cmd.test.output]`).
        Self::main_func(out, "(void)argc; (void)argv; em_start(0, NULL)", |w| {
            for ((name, _), (thunk, ret)) in tests.iter().zip(thunks) {
                w.stmt(&call("em_run", [thunk.as_str(), repr_code_of(ret)]));
                w.stmt(&call(
                    "em_test_result",
                    [names::string_literal(name.as_bytes())],
                ));
            }
            w.stmt("em_test_done()");
        });
    }

    pub(super) fn c_main(&mut self, out: &mut Writer, main: Option<FnId>) {
        self.cleanup(out);
        let thunk = main.map(|id| self.entry_thunk(out, id));
        Self::main_func(out, "em_start(argc, argv)", |w| {
            if let Some((thunk, ret)) = thunk {
                w.stmt(&call("em_run", [thunk.as_str(), repr_code_of(ret)]));
            }
        });
    }
}
