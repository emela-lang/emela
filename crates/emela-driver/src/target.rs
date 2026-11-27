//! Build targets (spec chapter 16). The default target of a project (`[build].target` in
//! `Pome.toml`) is read with the project.

/// What build, run and test compile to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Target {
    /// JS run on the JS host (Tier 1).
    #[default]
    Js,
    /// An executable for the machine that builds, made by emitting C and compiling it with the C
    /// compiler of that machine (Tier 2, `[target.native]`).
    Native,
    /// WebAssembly (WASI) run with `wasmtime` (Tier 1).
    Wasm,
}

impl Target {
    /// The names `--target` and `[build].target` accept. `c`, the name of the native target
    /// before 0.28, is an unknown target since 0.29 (`[target.native]`).
    pub const NAMES: [&'static str; 3] = ["js", "native", "wasm"];

    /// The target that `name` names.
    pub fn parse(name: &str) -> Option<Target> {
        match name {
            "js" => Some(Target::Js),
            "native" => Some(Target::Native),
            "wasm" => Some(Target::Wasm),
            _ => None,
        }
    }

    /// The name used by the CLI and `[build].target`.
    pub fn name(self) -> &'static str {
        match self {
            Target::Js => "js",
            Target::Native => "native",
            Target::Wasm => "wasm",
        }
    }

    /// The language of the code that the target links with: the first argument of the
    /// `@external` and `@export` that take effect on it (`[ext.decl]`).
    pub fn language(self) -> &'static str {
        match self {
            Target::Js => "js",
            Target::Native => "c",
            Target::Wasm => "wasm",
        }
    }

    /// The target whose language is `language` ([`Target::language`]).
    pub fn of_language(language: &str) -> Option<Target> {
        [Target::Js, Target::Native, Target::Wasm]
            .into_iter()
            .find(|t| t.language() == language)
    }
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}
