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

/// The runtime functions of internal hosts that read WASI's clock (`clock_time_get`). When one is
/// requested the import is registered (see `Builder::build`, `uses_timers`). This is the only list
/// that decides whether the clock import is emitted, so **a new clock host adds its runtime
/// function name here** (the third column of its [`INTERNAL_HOSTS`] row).
///
/// `clock_now_milliseconds` reads the REALTIME clock directly; the others go through the
/// scheduler's `now_ms`/`after` and are also in [`TIMER_HOSTS`]. The two concepts stay separate:
/// the clock import is what `uses_timers` gates, the scheduler globals are what `TIMER_HOSTS`
/// names (see [`Builder::build`]).
pub(crate) const CLOCK_HOSTS: &[&str] = &[
    "clock_now_milliseconds",
    "clock_sleep_milliseconds",
    "clock_when_stalled",
    "clock_sleep",
    "test_sleep_after",
];

/// The runtime functions of internal hosts that use the scheduler's **timers** (`after`/
/// `timers_due`, spec 12.6): they read and write the globals `G_TIMERS`/`G_TIMER_SEQ`, which only
/// a suspending module has. A host here is a `suspend fn` host (the task argument ties it to the
/// suspend ABI), so requesting one implies [`Builder::uses_suspend`]; `check` rejects a
/// `suspend fn` external bound to anything but an internal host, and [`Builder::build`] asserts
/// the invariant. The runtime names are all in [`CLOCK_HOSTS`] too, because `after` reads
/// `now_ms`, which reads WASI's clock.
pub(crate) const TIMER_HOSTS: &[&str] = &[
    "clock_sleep_milliseconds",
    "clock_when_stalled",
    "clock_sleep",
    "test_sleep_after",
];

/// The module name of a `HostModule` (`@external(wasm, "module", "name")`).
pub(crate) fn module_name(module: &HostModule) -> &str {
    match module {
        HostModule::Named(n) => n,
        HostModule::File(p) | HostModule::Bundled(p) => p,
    }
}

/// The `(module, name)` of `f`'s `@external(wasm, ...)` binding when it names a module/name
/// import (`None` for a global reference or no binding). Used by [`crate::check`] to reject a
/// `suspend fn` external bound to something other than an internal host.
pub(crate) fn wasm_import(f: &Function) -> Option<(String, String)> {
    match &f.external.as_ref()?.binding("wasm")?.host {
        HostRef::Import { module, name } => Some((module_name(module).to_owned(), name.clone())),
        HostRef::Global(_) => None,
    }
}

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
        wasm_import(f)
    }

    pub(super) fn external_import(
        &mut self,
        id: FnId,
        f: &Function,
        binding: &emela_core::ExternalBinding,
    ) {
        let (module, name) = match &binding.host {
            HostRef::Import { module, name } => (module_name(module).to_owned(), name.clone()),
            HostRef::Global(_) => {
                self.internal(format!(
                    "`@external(wasm, ...)` of `{}` is not a module/name import",
                    f.name
                ));
                return;
            }
        };
        // The internal host registry comes first: such a binding is a runtime function, not a
        // wasm import, and its (reference) parameters cross as `i32` pointers. The contract check
        // is on both paths: before the runtime index space is fixed (`internal_host` returns
        // `None`) the binding is remembered and `bind_internal_hosts` resolves it afterwards, and
        // the check must run just the same.
        if internal_host_name(&module, &name).is_some() {
            self.check_internal_host(f, &module, &name);
            match self.internal_host(&module, &name) {
                Some(index) => self.fn_index[ix(id)] = index,
                None => self.deferred_hosts.push((ix(id), module, name)),
            }
            return;
        }
        // A `suspend fn` external crosses only through an internal host: the runtime function
        // takes the task last and completes it (spec 12.6). A plain wasm import would be called
        // synchronously and drop the suspension. `check` reports this as `SuspendExternal`; this
        // keeps the emitter from writing the synchronous call if it is reached anyway.
        if f.suspends {
            self.errors.push(WasmError::new(
                WasmErrorKind::SuspendExternal,
                format!(
                    "`suspend fn` external function `{}` is bound to the WASM import \
                     `{module}`.`{name}`, which is not an internal host; only an internal host can \
                     suspend",
                    f.name
                ),
            ));
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
        // The result crosses too: `()` is nothing, a scalar its `host_val`, and anything else (a
        // `String`, a list, …) is reported like an unsupported parameter.
        let results: Vec<ValType> = if self.repr(f.ret) == Repr::Void {
            Vec::new()
        } else {
            match self.host_val(f.ret) {
                Some(v) => vec![v],
                None => {
                    self.host_type_error(f, f.ret);
                    return;
                }
            }
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
