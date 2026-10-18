//! The external-function boundary: the wasm imports, the internal host registry (see
//! [`INTERNAL_HOSTS`]) and the stubs for an external with no binding on this target.
//!
//! # The internal host ABI
//!
//! An `@external(wasm, "module", "name")` binding is normally a real wasm import whose signature
//! must be one of the scalar types [`host_val`](Builder::host_val) accepts (`Int` / `Bool` /
//! `Int64` / `Float`) — a `String`, list or any other reference cannot cross the boundary yet.
//!
//! An *internal host* is different: its `(module, name)` is listed in [`INTERNAL_HOSTS`] and it
//! binds to a runtime function emitted into the same module instead of to a wasm import. That
//! function's signature is the [`repr_val`] image of the LIR signature, so a `String`, a list or
//! any other reference parameter or result becomes an `i32` (a pointer into linear memory) and
//! `()` becomes nothing. The runtime function reads linear memory directly, so no marshalling
//! happens at the call.
//!
//! This is the seam every later host (`Std.Console`, `Std.Clock`, `Std.Env`, `Std.Random`,
//! `Std.FileSystem`) uses: add the binding to [`INTERNAL_HOSTS`], append the runtime function to
//! the fragment that owns it (`runtime::host`, `runtime::fs`, …) and it is emitted on demand.
//! Programs that use no internal host keep the exact module bytes they had before the runtime
//! function existed (see `runtime::is_on_demand`).

use super::*;

/// `(wasm module, wasm name)` -> runtime function name in `runtime::FUNCS`.
///
/// The third element must be a runtime function whose wasm signature is the `repr_val` image of
/// the `fn`'s LIR signature (references and `String` become `i32`). It is emitted only when a
/// program references it.
pub(crate) const INTERNAL_HOSTS: &[(&str, &str, &str)] = &[
    ("test", "print", "print_string"),
    ("emela:console", "log", "console_log"),
    ("emela:console", "error", "console_error"),
    ("emela:env", "has", "env_has"),
    ("emela:env", "get", "env_get"),
    ("emela:random", "int", "random_int"),
    ("emela:random", "float", "random_float"),
    // The `suspend fn` external hosts of the WASM tests (`tests/suspend_external.rs`): one
    // completes in place, one uses the driver's timers.
    ("emela:test", "sleep_ms", "test_sleep_ms"),
    ("emela:test", "sleep_after", "test_sleep_after"),
    // `Std.FileSystem` (spec 15.2). The `Outcome` an operation returns and the accessors that
    // take it apart are opaque handles; see `emit::handle`.
    ("emela:fs", "read_text", "fs_read_text"),
    ("emela:fs", "write_text", "fs_write_text"),
    ("emela:fs", "append_text", "fs_append_text"),
    ("emela:fs", "exists", "fs_exists"),
    ("emela:fs", "is_directory", "fs_is_directory"),
    ("emela:fs", "list_directory", "fs_list_directory"),
    ("emela:fs", "create_directory", "fs_create_directory"),
    ("emela:fs", "remove", "fs_remove"),
    ("emela:fs", "outcome_error", "fs_outcome_error"),
    ("emela:fs", "outcome_message", "fs_outcome_message"),
    ("emela:fs", "outcome_text", "fs_outcome_text"),
    ("emela:fs", "outcome_name_count", "fs_outcome_name_count"),
    ("emela:fs", "outcome_name_at", "fs_outcome_name_at"),
    ("emela:fs", "release_outcome", "fs_release_outcome"),
    // `Std.Clock` (spec 8.1, 15.2). `now_milliseconds` reads the REALTIME clock; the suspending
    // hosts (`sleep_milliseconds`, `when_stalled` and the corpus' `sleep`) register a driver timer
    // and the driver resumes the fiber with its value.
    ("emela:clock", "now_milliseconds", "clock_now_milliseconds"),
    (
        "emela:clock",
        "sleep_milliseconds",
        "clock_sleep_milliseconds",
    ),
    ("emela:clock", "when_stalled", "clock_when_stalled"),
    ("emela:clock", "sleep", "clock_sleep"),
];

/// If `(module, name)` is an internal host, the runtime function it binds to.
pub(crate) fn internal_host_name(module: &str, name: &str) -> Option<&'static str> {
    INTERNAL_HOSTS
        .iter()
        .find(|(m, n, _)| *m == module && *n == name)
        .map(|(_, _, runtime)| *runtime)
}

/// The runtime functions of internal hosts that use the scheduler's timers (`after`/`now_ms`,
/// spec 12.6). When one is requested the WASI `clock_time_get` import is registered (see
/// `Builder::build`). **T-Clock adds its `sleep` runtime function here** (the name in the third
/// column of its [`INTERNAL_HOSTS`] row) so the module gets the clock.
pub(crate) const TIMER_HOSTS: &[&str] = &[
    "clock_sleep_milliseconds",
    "sleep_milliseconds",
    "sleep",
    "sleep_ms",
    "when_stalled",
    "test_sleep_after",
    // T-Clock's runtime function names (see `runtime::clock`). All of them read the WASI clock
    // (`now_milliseconds` directly; the suspending hosts through the driver's timers), so a
    // program using any of them imports `clock_time_get`.
    "clock_now_milliseconds",
    "clock_when_stalled",
    "clock_sleep",
];

impl<'m> Builder<'m> {
    /// The wasm function index of the runtime function an internal host binds to, recording the
    /// request so `runtime::register` emits it.
    ///
    /// The runtime index space is only fixed after the IR functions have their slots (the runtime
    /// is registered after them), so before that this returns `None` and the caller defers the
    /// binding to [`bind_internal_hosts`](Builder::bind_internal_hosts); afterwards it returns the
    /// index.
    pub(crate) fn internal_host(&mut self, module: &str, name: &str) -> Option<u32> {
        let runtime = internal_host_name(module, name)?;
        if !self.requested_hosts.contains(&runtime) {
            self.requested_hosts.push(runtime);
        }
        self.rt.get(runtime).copied()
    }

    /// Fills in the `fn_index` of the internal hosts seen while the runtime index space was still
    /// open (called right after `runtime::register`).
    pub(super) fn bind_internal_hosts(&mut self) {
        for (slot, module, name) in std::mem::take(&mut self.deferred_hosts) {
            match self.internal_host(&module, &name) {
                Some(index) => self.fn_index[slot] = index,
                None => self.internal(format!(
                    "internal host `{module}.{name}` has no runtime function"
                )),
            }
        }
    }

    /// Checks that the internal host `(module, name)`'s LIR signature maps (through the
    /// representation of each parameter and of the result; a reference is an `i32` pointer) to
    /// exactly the signature of the runtime function it binds to.
    fn check_internal_host(&mut self, f: &Function, module: &str, name: &str) {
        let Some(host) = internal_host_name(module, name) else {
            return;
        };
        let params: Vec<ValType> = f.params.iter().map(|&p| val(f.vars[p].repr)).collect();
        let results: Vec<ValType> = repr_val(self.repr(f.ret)).into_iter().collect();
        // A `suspend fn` host either returns its value directly (an in-place host, e.g. `Std.FileSystem`'s
        // `host_*`) or takes the task (`i32`) last and completes it instead of returning (the wrapper
        // appends the task; a host that suspends, e.g. T-Clock's `sleep`; spec 12.6).
        let matches = |host_params: &[ValType], host_results: &[ValType]| {
            if host_params == params.as_slice() && host_results == results.as_slice() {
                return true;
            }
            if f.suspends {
                let mut with_task = params.clone();
                with_task.push(ValType::I32);
                return host_params == with_task.as_slice() && host_results.is_empty();
            }
            false
        };
        match super::runtime::signature(host) {
            Some((host_params, host_results)) if matches(host_params, host_results) => {}
            other => self.internal(format!(
                "internal host `{module}.{name}` takes {params:?} -> {results:?}, but the runtime \
                 function `{host}` is {other:?}"
            )),
        }
    }

    /// The `(module, name)` of an external function's `@external(wasm, ...)` import, if any.
    pub(super) fn wasm_host(&self, f: &Function) -> Option<(String, String)> {
        let binding = f.external.as_ref()?.binding("wasm")?;
        match &binding.host {
            HostRef::Import { module, name } => {
                let module = match module {
                    HostModule::Named(n) => n.clone(),
                    HostModule::File(p) | HostModule::Bundled(p) => p.clone(),
                };
                Some((module, name.clone()))
            }
            HostRef::Global(_) => None,
        }
    }

    pub(super) fn external_import(
        &mut self,
        id: FnId,
        f: &Function,
        binding: &emela_core::ExternalBinding,
    ) {
        let (module, name) = match &binding.host {
            HostRef::Import { module, name } => {
                let module = match module {
                    HostModule::Named(n) => n.clone(),
                    HostModule::File(p) | HostModule::Bundled(p) => p.clone(),
                };
                (module, name.clone())
            }
            HostRef::Global(_) => {
                self.internal(format!(
                    "`@external(wasm, ...)` of `{}` is not a module/name import",
                    f.name
                ));
                return;
            }
        };
        // The internal host registry comes first: such a binding is a runtime function, not a
        // wasm import, and its (reference) parameters cross as `i32` pointers.
        if let Some(index) = self.internal_host(&module, &name) {
            self.fn_index[ix(id)] = index;
            return;
        }
        if internal_host_name(&module, &name).is_some() {
            // The runtime index space is fixed after the IR functions; remember the binding.
            self.check_internal_host(f, &module, &name);
            self.deferred_hosts.push((ix(id), module, name));
            return;
        }
        let mut params = Vec::new();
        for &p in &f.params {
            match self.host_val(f.vars[p].ty) {
                Some(v) => params.push(v),
                None => {
                    self.host_type_error(f, f.vars[p].ty);
                    return;
                }
            }
        }
        let results: Vec<ValType> = match self.host_val(f.ret) {
            Some(v) => vec![v],
            None => vec![],
        };
        let ty = self.ty(params, results);
        if let Some(&i) = self.import_map.get(&(module.clone(), name.clone())) {
            self.fn_index[ix(id)] = i;
            return;
        }
        let index = self.imports.len() as u32;
        self.imports.push((module.clone(), name.clone(), ty));
        self.import_map.insert((module, name), index);
        self.fn_index[ix(id)] = index;
    }

    pub(super) fn host_val(&self, ty: TypeId) -> Option<ValType> {
        Some(match self.core().ty(ty) {
            Type::Int | Type::Bool => ValType::I32,
            Type::Int64 => ValType::I64,
            Type::Float => ValType::F64,
            _ => return None,
        })
    }

    pub(super) fn host_type_error(&mut self, f: &Function, ty: TypeId) {
        let name = emela_lir::core_ty(self.core(), ty);
        let kind = if matches!(self.core().ty(ty), Type::Fn(_)) {
            WasmErrorKind::Callback
        } else {
            WasmErrorKind::Unsupported
        };
        let message = if kind == WasmErrorKind::Callback {
            format!(
                "external function `{}` takes a callback, which target `wasm` does not support yet",
                f.name
            )
        } else {
            format!(
                "external function `{}` uses the type {name}, which cannot cross to WASM",
                f.name
            )
        };
        self.errors.push(WasmError::new(kind, message));
    }

    pub(super) fn external_stub(&mut self, _id: FnId, f: &Function) -> WasmFunction {
        let msg = format!(
            "external function `{}` is not available on this target",
            check::display_name(&f.name)
        );
        let s = self.string_literal(msg.as_bytes());
        let ret = self.repr(f.ret);
        let mut func = WasmFunction::new(Vec::<(u32, ValType)>::new());
        {
            let mut ins = func.instructions();
            ins.i32_const(s as i32);
            let raise = self.rt("raise");
            ins.call(raise);
            dummy_return(&mut ins, ret);
            ins.end();
        }
        func
    }
}
