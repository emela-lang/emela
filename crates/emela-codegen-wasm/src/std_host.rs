//! The WASI 0.3 host functions the component target knows how to call.
//!
//! A program's own `@external(wasm, ...)` is not a component import: a component can only import
//! interfaces its WIT world declares, and custom host WIT is not implemented. The bundled `Std`
//! modules do name a few WASI 0.3 functions, though (`Std.Console` stdout/stderr, `Std.Env`,
//! `Std.Random` and `Std.Clock.now`), and those are the only `wasm` bindings the target accepts.
//! [`classify`] is the allow list: a named module/name pair plus the shape of the Emela function.
//! Everything else stays an unsupported external in `check/external.rs`.

use emela_core::{HostModule, HostRef, Type};
use emela_lir::{Function, Module};

/// The imported interfaces a `wasm` binding may name. The function signatures are checked by
/// [`classify`], so a matching pair with another shape is still rejected.
pub(crate) const ALLOWED: &[(&str, &str)] = &[
    ("wasi:cli/stdout@0.3.0", "write-via-stream"),
    ("wasi:cli/stderr@0.3.0", "write-via-stream"),
    ("wasi:cli/environment@0.3.0", "get-environment"),
    ("wasi:random/random@0.3.0", "get-random-u64"),
    ("wasi:clocks/system-clock@0.3.0", "now"),
];

/// A supported WASI 0.3 host call, and the runtime wrapper that implements it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StdHost {
    /// `Std.Console.print_line`: a String, then a newline, on stdout.
    ConsoleOut,
    /// `Std.Console.print_error_line`: a String, then a newline, on stderr.
    ConsoleErr,
    /// `Std.Env.has`: whether the environment holds the name.
    EnvHas,
    /// `Std.Env.get`: the value of the name.
    EnvGet,
    /// `Std.Random.int`: 32 random bits.
    RandomInt,
    /// `Std.Random.float`: uniform in `[0, 1)`.
    RandomFloat,
    /// `Std.Clock.now`: Unix epoch milliseconds.
    ClockNow,
}

impl StdHost {
    pub(crate) fn runtime_name(self) -> &'static str {
        match self {
            StdHost::ConsoleOut => "print_line",
            StdHost::ConsoleErr => "print_error_line",
            StdHost::EnvHas => "env_has",
            StdHost::EnvGet => "env_get",
            StdHost::RandomInt => "random_int",
            StdHost::RandomFloat => "random_float",
            StdHost::ClockNow => "clock_now",
        }
    }

    pub(crate) fn is_console(self) -> bool {
        matches!(self, StdHost::ConsoleOut | StdHost::ConsoleErr)
    }

    pub(crate) fn is_env(self) -> bool {
        matches!(self, StdHost::EnvHas | StdHost::EnvGet)
    }

    pub(crate) fn is_random(self) -> bool {
        matches!(self, StdHost::RandomInt | StdHost::RandomFloat)
    }

    pub(crate) fn is_clock(self) -> bool {
        matches!(self, StdHost::ClockNow)
    }
}

/// If `f` is an external with a supported WASI 0.3 `wasm` binding, which host call it is.
pub(crate) fn classify(m: &Module, f: &Function) -> Option<StdHost> {
    if f.suspends {
        return None;
    }
    let binding = f.external.as_ref()?.binding("wasm")?;
    let HostRef::Import { module, name } = &binding.host else {
        return None;
    };
    let HostModule::Named(module) = module else {
        return None;
    };
    if !ALLOWED.contains(&(module.as_str(), name.as_str())) {
        return None;
    }
    let host = match (module.as_str(), name.as_str()) {
        ("wasi:cli/stdout@0.3.0", "write-via-stream") => StdHost::ConsoleOut,
        ("wasi:cli/stderr@0.3.0", "write-via-stream") => StdHost::ConsoleErr,
        ("wasi:cli/environment@0.3.0", "get-environment") => match m.core.ty(f.ret) {
            Type::Bool => StdHost::EnvHas,
            Type::String => StdHost::EnvGet,
            _ => return None,
        },
        ("wasi:random/random@0.3.0", "get-random-u64") => match m.core.ty(f.ret) {
            Type::Int => StdHost::RandomInt,
            Type::Float => StdHost::RandomFloat,
            _ => return None,
        },
        ("wasi:clocks/system-clock@0.3.0", "now") => StdHost::ClockNow,
        _ => return None,
    };
    if signature_matches(m, f, host) {
        Some(host)
    } else {
        None
    }
}

/// Whether `f` has exactly the Emela-side shape of `host`.
fn signature_matches(m: &Module, f: &Function, host: StdHost) -> bool {
    let takes = |ty: Type| f.params.len() == 1 && *m.core.ty(f.vars[f.params[0]].ty) == ty;
    let takes_string = || takes(Type::String);
    let ret = |ty: Type| *m.core.ty(f.ret) == ty;
    match host {
        StdHost::ConsoleOut | StdHost::ConsoleErr => takes_string() && ret(Type::Unit),
        StdHost::EnvHas => takes_string() && ret(Type::Bool),
        StdHost::EnvGet => takes_string() && ret(Type::String),
        StdHost::RandomInt => f.params.is_empty() && ret(Type::Int),
        StdHost::RandomFloat => f.params.is_empty() && ret(Type::Float),
        StdHost::ClockNow => f.params.is_empty() && ret(Type::Int64),
    }
}
