//! External functions (spec 12.4): how their values cross to C and the bodies that call the host.

use super::*;

/// How an external function's value crosses to C (spec 12.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HostTy {
    Int,
    Int64,
    Float,
    Bool,
    String,
    /// Two C parameters, the bytes and their number, and `em_bytes` as a return value.
    Bytes,
    Unit,
    Handle,
}

impl HostTy {
    /// The C types of the parameters a value of this type takes, in order.
    pub(super) fn params(self) -> &'static [&'static str] {
        match self {
            HostTy::Int => &["int32_t"],
            HostTy::Int64 => &["int64_t"],
            HostTy::Float => &["double"],
            HostTy::Bool => &["bool"],
            HostTy::String => &["const char *"],
            HostTy::Bytes => &["const uint8_t *", "size_t"],
            HostTy::Unit => &["void"],
            HostTy::Handle => &["void *"],
        }
    }

    pub(super) fn ret(self) -> &'static str {
        match self {
            HostTy::String => "char *",
            HostTy::Bytes => "em_bytes",
            other => other.params()[0],
        }
    }

    /// The suffixes of the names of the C parameters a value of this type takes, in order.
    pub(super) fn suffixes(self) -> &'static [&'static str] {
        match self {
            HostTy::Bytes => &["_data", "_len"],
            _ => &[""],
        }
    }
}

impl Emitter<'_> {
    /// An external function: converts the arguments for C, calls the host function and converts
    /// the result back (spec 12.4). The arguments are borrowed.
    pub(super) fn external(
        &mut self,
        out: &mut Writer,
        id: FnId,
        f: &Function,
        ext: &emela_core::External,
    ) {
        out.comment(&format!("external {}", f.name));
        let sig = self.signature(id, f, "");
        out.static_func(&sig, |w| self.external_body(w, f, ext));
    }

    pub(super) fn external_body(
        &mut self,
        w: &mut Writer,
        f: &Function,
        ext: &emela_core::External,
    ) {
        let ret = self.repr(f.ret);
        let dummy = dummy(CTy::Scalar(ret), &self.values);
        let dummy = dummy.as_deref();
        let Some(binding) = ext.binding("c") else {
            let msg = format!(
                "external function `{}` is not available on this target",
                display_name(&f.name)
            );
            w.stmt(&call(
                "em_raise_cstr",
                [names::string_literal(msg.as_bytes())],
            ));
            w.ret(dummy);
            return;
        };
        let host = match &binding.host {
            HostRef::Global(name) => name.clone(),
            HostRef::Import { module, name } => {
                match module {
                    HostModule::Bundled(path) => {
                        if !self.bundled.contains(path) {
                            self.bundled.push(path.clone());
                        }
                    }
                    HostModule::File(path) => {
                        if !self.host_files.contains(path) {
                            self.host_files.push(path.clone());
                        }
                    }
                    HostModule::Named(_) => {}
                }
                name.clone()
            }
        };
        if !names::is_c_identifier(&host) {
            self.errors.push(CError::new(
                CErrorKind::Unsupported,
                format!(
                    "`{host}` in `@external(c, ...)` of `{}` is not a C function name",
                    f.name
                ),
            ));
        }
        let mut params = Vec::new();
        for &p in &f.params {
            match self.host_ty(f.vars[p].ty) {
                Some(t) => params.push((p, t)),
                None => {
                    self.host_type_error(f, f.vars[p].ty);
                    return;
                }
            }
        }
        let ret_ty = if f.ret == TypeId::UNIT || f.ret == TypeId::NEVER {
            HostTy::Unit
        } else {
            match self.host_ty(f.ret) {
                Some(t) => t,
                None => {
                    self.host_type_error(f, f.ret);
                    return;
                }
            }
        };
        if self.host_names.insert(host.clone()) {
            let mut ps: Vec<&str> = params
                .iter()
                .flat_map(|(_, t)| t.params().iter().copied())
                .collect();
            // A `suspend fn` takes the task last and completes it later (spec 12.6).
            if f.suspends {
                ps.push("em_task *");
            }
            let ret = if f.suspends { "void" } else { ret_ty.ret() };
            self.host_protos.extern_proto(&signature(ret, &host, ps));
        }
        let mut host_args = Vec::new();
        for (i, (p, t)) in params.iter().enumerate() {
            let v = var(*p);
            match t {
                HostTy::String => {
                    let c = format!("c{i}");
                    w.decl("const char *", &c, Some(&call("em_cstr", [&v])));
                    w.if_return(&format!("{c} == NULL"), dummy);
                    host_args.push(c);
                }
                // The runtime's own bytes, without a copy (`[ext.boundary]`).
                HostTy::Bytes => {
                    host_args.push(call("em_bytes_data", [&v]));
                    host_args.push(call("em_bytes_len", [&v]));
                }
                HostTy::Bool => host_args.push(format!("({v} != 0)")),
                HostTy::Handle => host_args.push(call("em_host_get", [&v])),
                _ => host_args.push(v),
            }
        }
        let name = names::string_literal(display_name(&f.name).as_bytes());
        if f.suspends {
            let kind = match ret_ty {
                HostTy::Unit => "EM_HOST_UNIT",
                HostTy::Int => "EM_HOST_INT",
                HostTy::Int64 => "EM_HOST_INT64",
                HostTy::Float => "EM_HOST_FLOAT",
                HostTy::Bool => "EM_HOST_BOOL",
                HostTy::String => "EM_HOST_STRING",
                HostTy::Bytes => "EM_HOST_BYTES",
                HostTy::Handle => "EM_HOST_HANDLE",
            };
            w.decl(
                "em_task *",
                "task",
                Some(&call("em_task_new", [kind, name.as_str()])),
            );
            // The host may use the bytes of a String or Bytes argument until the task completes,
            // after the waiting fiber has been interrupted and its frames dropped the arguments
            // too (`[ext.boundary]`), so the task holds a reference to each.
            for (p, t) in &params {
                if matches!(t, HostTy::String | HostTy::Bytes) {
                    w.stmt(&call("em_task_keep", ["task".to_owned(), var(*p)]));
                }
            }
            host_args.push("task".to_owned());
            w.stmt(&call(&host, &host_args));
            if ret == Repr::Void {
                w.stmt("em_task_end(task)");
            } else {
                w.ret(Some(&format!("em_task_end(task).{}", acc(ret))));
            }
            return;
        }
        let host_call = call(&host, &host_args);
        match ret_ty {
            HostTy::Unit => {
                w.stmt(&host_call);
                w.ret(dummy);
            }
            HostTy::String => {
                w.ret(Some(&call("em_from_host_string", [host_call, name])));
            }
            HostTy::Bytes => {
                w.ret(Some(&call("em_from_host_bytes", [host_call, name])));
            }
            HostTy::Bool => w.ret(Some(&format!("{host_call} ? 1 : 0"))),
            HostTy::Handle => w.ret(Some(&call("em_host_box", [host_call]))),
            _ => w.ret(Some(&host_call)),
        }
    }

    pub(super) fn host_ty(&self, ty: TypeId) -> Option<HostTy> {
        let core = self.core();
        Some(match core.ty(ty) {
            Type::Int => HostTy::Int,
            Type::Int64 => HostTy::Int64,
            Type::Float => HostTy::Float,
            Type::Bool => HostTy::Bool,
            Type::String => HostTy::String,
            Type::Bytes => HostTy::Bytes,
            Type::Unit | Type::Never => HostTy::Unit,
            Type::Enum(id, _) if core.enums[*id].external => HostTy::Handle,
            _ => return None,
        })
    }

    pub(super) fn host_type_error(&mut self, f: &Function, ty: TypeId) {
        let name = emela_lir::core_ty(self.core(), ty);
        let (kind, what) = if matches!(self.core().ty(ty), Type::Fn(_)) {
            (
                CErrorKind::Callback,
                format!(
                    "external function `{}` takes a callback, which target `native` does not support yet",
                    f.name
                ),
            )
        } else {
            (
                CErrorKind::Unsupported,
                format!(
                    "external function `{}` uses the type {name}, which cannot cross to C",
                    f.name
                ),
            )
        };
        self.errors.push(CError::new(kind, what));
    }
}
