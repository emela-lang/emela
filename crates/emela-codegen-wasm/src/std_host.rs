//! The WASI 0.3 host functions the component target knows how to call.
//!
//! A program's own `@external(wasm, ...)` is not a component import: a component can only import
//! interfaces its WIT world declares, and custom host WIT is not implemented. The bundled `Std`
//! modules do name a few WASI 0.3 functions (`Std.Console` stdout/stderr, `Std.Env`,
//! `Std.Random` and `Std.Clock.now`); each of those externals is a runtime wrapper with its own
//! encoding of the host's result. [`classify`] recognizes them by their IR name, never by the
//! module/name pair, so a program that names the same WASI function stays an unsupported
//! external in `check/external.rs`.

use emela_lir::Function;

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

/// The bundled externals that are runtime wrappers, by IR name. `Std` cannot be the root name of
/// a project or a dependency (spec 4.1), so these names only come from the bundled sources.
const STD_HOSTS: &[(&str, StdHost)] = &[
    ("Std.Console.console_log", StdHost::ConsoleOut),
    ("Std.Console.console_error", StdHost::ConsoleErr),
    ("Std.Env.env_has", StdHost::EnvHas),
    ("Std.Env.env_get", StdHost::EnvGet),
    ("Std.Random.random_int", StdHost::RandomInt),
    ("Std.Random.random_float", StdHost::RandomFloat),
    ("Std.Clock.now_milliseconds", StdHost::ClockNow),
];

/// If `f` is a bundled `Std` external with a `wasm` binding that the runtime implements, which
/// host call it is.
pub(crate) fn classify(f: &Function) -> Option<StdHost> {
    f.external.as_ref()?.binding("wasm")?;
    STD_HOSTS
        .iter()
        .find(|(name, _)| *name == f.name)
        .map(|&(_, host)| host)
}
