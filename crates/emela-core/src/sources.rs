//! The Prelude, core and std sources bundled with the compiler (spec 15.2).
//!
//! The sources are the `.emel` files under `lib/core/` and `lib/std/`, embedded with
//! `include_str!`. Functions that cannot be written in Emela are declared as bodiless fns marked
//! `@intrinsic`, and their implementation is looked up in the builtin table via
//! `Builtin::lookup_intrinsic`.

/// The module name of the Prelude source. `prelude.emel` is reserved at the source root (spec 4.1).
pub const PRELUDE: &str = "Prelude";

/// The namespace for std modules (spec 15.2). std modules have names like
/// `Std.Console` and `Std.Embedded.Gpio` and are imported. `std.emel` and `std/` directly under
/// the source root are reserved (spec 4.1).
pub const STD: &str = "Std";

/// Bundled sources as (module name, source) pairs, Prelude first. core (`lib/core/`, short names
/// usable without import) comes before std (`lib/std/`, under `Std`).
const SOURCES: &[(&str, &str)] = &[
    (PRELUDE, include_str!("../lib/core/prelude.emel")),
    ("List", include_str!("../lib/core/list.emel")),
    ("Option", include_str!("../lib/core/option.emel")),
    ("String", include_str!("../lib/core/string.emel")),
    ("Int", include_str!("../lib/core/int.emel")),
    ("Int64", include_str!("../lib/core/int64.emel")),
    ("Float", include_str!("../lib/core/float.emel")),
    ("Bool", include_str!("../lib/core/bool.emel")),
    ("Std.Duration", include_str!("../lib/std/duration.emel")),
    ("Std.Instant", include_str!("../lib/std/instant.emel")),
    ("Std.Map", include_str!("../lib/std/map.emel")),
    ("Std.Set", include_str!("../lib/std/set.emel")),
    ("Std.State", include_str!("../lib/std/state.emel")),
    ("Std.Console", include_str!("../lib/std/console.emel")),
    ("Std.Random", include_str!("../lib/std/random.emel")),
    ("Std.Env", include_str!("../lib/std/env.emel")),
    ("Std.Clock", include_str!("../lib/std/clock.emel")),
    (
        "Std.Http.Request",
        include_str!("../lib/std/http/request.emel"),
    ),
    (
        "Std.Http.Response",
        include_str!("../lib/std/http/response.emel"),
    ),
    ("Std.Http", include_str!("../lib/std/http.emel")),
    ("Std.Path", include_str!("../lib/std/path.emel")),
    (
        "Std.FileSystem",
        include_str!("../lib/std/file_system.emel"),
    ),
    ("Std.Platform", include_str!("../lib/std/platform.emel")),
    ("Std.Cause", include_str!("../lib/std/cause.emel")),
    ("Std.Deferred", include_str!("../lib/std/deferred.emel")),
    ("Std.Fiber", include_str!("../lib/std/fiber.emel")),
    (
        "Std.Embedded.Peripheral",
        include_str!("../lib/std/embedded/peripheral.emel"),
    ),
    (
        "Std.Embedded.Gpio",
        include_str!("../lib/std/embedded/gpio.emel"),
    ),
    (
        "Std.Embedded.Uart",
        include_str!("../lib/std/embedded/uart.emel"),
    ),
    (
        "Std.Embedded.I2c",
        include_str!("../lib/std/embedded/i2c.emel"),
    ),
    (
        "Std.Embedded.Spi",
        include_str!("../lib/std/embedded/spi.emel"),
    ),
    (
        "Std.Embedded.Board",
        include_str!("../lib/std/embedded/board.emel"),
    ),
];

/// Bundled host files as (path relative to `lib/`, contents) pairs. These are the files that
/// `@external(js, "./x.mjs", …)` and `@external(c, "./x.c", …)` in bundled sources point to,
/// embedded with `include_str!` like the `.emel` files (see `lib/README.md` for the procedure).
const HOST_FILES: &[(&str, &str)] = &[
    ("std/random.mjs", include_str!("../lib/std/random.mjs")),
    ("std/env.mjs", include_str!("../lib/std/env.mjs")),
    ("std/clock.mjs", include_str!("../lib/std/clock.mjs")),
    ("std/http.mjs", include_str!("../lib/std/http.mjs")),
    (
        "std/file_system.mjs",
        include_str!("../lib/std/file_system.mjs"),
    ),
    ("std/console.c", include_str!("../lib/std/console.c")),
    ("std/random.c", include_str!("../lib/std/random.c")),
    ("std/env.c", include_str!("../lib/std/env.c")),
    ("std/clock.c", include_str!("../lib/std/clock.c")),
    (
        "std/file_system.c",
        include_str!("../lib/std/file_system.c"),
    ),
];

/// The directory under `lib/` holding a bundled source: `std` for std modules, `core` otherwise.
/// `@external` file specifiers resolve relative to this directory.
pub fn lib_dir(module: &str) -> &'static str {
    match module.split_once('.') {
        Some((STD, _)) => "std",
        _ => "core",
    }
}

/// Prelude first.
pub fn core_sources() -> &'static [(&'static str, &'static str)] {
    SOURCES
}

/// Bundled host files as (path relative to `lib/`, contents) pairs.
pub fn bundled_files() -> &'static [(&'static str, &'static str)] {
    HOST_FILES
}

pub fn core_source(module: &str) -> Option<&'static str> {
    SOURCES.iter().find(|(m, _)| *m == module).map(|(_, s)| *s)
}

#[cfg(test)]
mod tests {
    use emela_syntax::ast::{AstNode, FnDecl, HasAttrs};
    use emela_syntax::{DiagnosticCode, SyntaxKind, SyntaxNode, parse};

    use super::*;
    use crate::{Builtin, Sig};

    fn fn_decls(src: &str) -> Vec<FnDecl> {
        parse(src).tree().fn_decls().collect()
    }

    fn name(f: &FnDecl) -> String {
        f.name().expect("a function has a name").text().to_owned()
    }

    /// The type as written, without whitespace.
    fn text(node: &SyntaxNode) -> String {
        node.text().to_string().split_whitespace().collect()
    }

    /// Renders a builtin table type in Emela syntax, with type parameters `A`, `B`, ….
    fn render(sig: Sig) -> String {
        match sig {
            Sig::Int => "Int".into(),
            Sig::Int64 => "Int64".into(),
            Sig::Float => "Float".into(),
            Sig::Bool => "Bool".into(),
            Sig::String => "String".into(),
            Sig::Unit => "()".into(),
            Sig::Never => "Never".into(),
            Sig::List(t) => format!("List[{}]", render(*t)),
            Sig::Option(t) => format!("Option[{}]", render(*t)),
            Sig::Param(i) => char::from(b'A' + i as u8).to_string(),
        }
    }

    #[test]
    fn sources_parse_without_syntax_errors() {
        for (module, src) in core_sources() {
            let parse = parse(src);
            let intrinsics: Vec<_> = fn_decls(src)
                .into_iter()
                .filter(|f| f.has_annotation("intrinsic"))
                .map(|f| f.syntax().text_range())
                .collect();
            // E0130 on `@intrinsic` declarations is allowed, from when the syntax check required
            // `@external` on every bodiless fn. It exempts `@intrinsic` now. Any other diagnostic
            // is an error.
            let errors: Vec<_> = parse
                .diagnostics()
                .iter()
                .filter(|d| {
                    !(d.code == DiagnosticCode::MissingExternal
                        && intrinsics.iter().any(|r| r.contains_range(d.range)))
                })
                .collect();
            assert!(errors.is_empty(), "syntax errors in {module}: {errors:#?}");
            let errors = parse
                .syntax()
                .descendants()
                .filter(|n| n.kind() == SyntaxKind::ERROR)
                .count();
            assert_eq!(errors, 0, "{module} has ERROR nodes");
        }
    }

    /// Fiber intrinsics (spec chapter 11). Lowering maps them to `Expr::Fiber` in the Core IR
    /// (`FiberFn` in `lower/fiber.rs`), so they have no row in the builtin table.
    const FIBER_INTRINSICS: &[(&str, &str)] = &[
        ("Std.Fiber", "scope"),
        ("Std.Fiber", "join"),
        ("Std.Fiber", "await"),
        ("Std.Fiber", "interrupt"),
        ("Std.Fiber", "attempt"),
        ("Std.Cause", "kind"),
        ("Std.Cause", "to_string"),
        ("Std.Cause", "raise"),
        ("Std.Deferred", "complete"),
        ("Std.Deferred", "await"),
        ("Std.Deferred", "poll"),
    ];

    /// Every row is declared exactly once, with the table's parameter and return types and number
    /// of type parameters. Parameter names (the labels of named arguments) are not in the table:
    /// the source is the authority for them.
    #[test]
    fn intrinsics_match_the_table() {
        let mut declared = Vec::new();
        for (module, src) in core_sources() {
            for f in fn_decls(src) {
                let name = name(&f);
                if !f.has_annotation("intrinsic") {
                    assert!(
                        f.body().is_some() || f.has_annotation("external"),
                        "{module}.{name} has no body"
                    );
                    continue;
                }
                assert!(f.body().is_none(), "@intrinsic {module}.{name} has a body");
                // Runtime handlers (`State.run`) are mapped by lowering on the spot, and fiber
                // intrinsics (spec chapter 11) are mapped to `Expr::Fiber`, so neither has a row
                // in the table.
                if crate::intrinsics::is_runtime_handler(module, &name)
                    || FIBER_INTRINSICS.contains(&(*module, name.as_str()))
                {
                    continue;
                }
                let op = Builtin::lookup_intrinsic(module, &name)
                    .unwrap_or_else(|| panic!("{module}.{name} is not in the builtin table"));
                let info = op.info();
                let params: Vec<_> = f
                    .param_list()
                    .into_iter()
                    .flat_map(|l| l.params().collect::<Vec<_>>())
                    .map(|p| text(&p.ty().expect("a parameter has a type")))
                    .collect();
                let want: Vec<_> = info.params.iter().map(|s| render(*s)).collect();
                assert_eq!(params, want, "parameter types of {module}.{name}");
                let ret = f.ret_type().map(|t| text(&t));
                assert_eq!(
                    ret,
                    Some(render(info.ret)),
                    "return type of {module}.{name}"
                );
                let ty_params = f
                    .type_params()
                    .map_or(0, |list| list.children().count() as u32);
                assert_eq!(
                    ty_params, info.ty_params,
                    "type parameter count of {module}.{name}"
                );
                declared.push(op);
            }
        }
        for op in Builtin::ALL {
            let n = declared.iter().filter(|d| *d == op).count();
            assert_eq!(n, 1, "{n} @intrinsic declarations of {}", op.info().path());
        }
    }

    /// Every `.mjs` and `.c` under `lib/` must be listed in `HOST_FILES`; otherwise an installed
    /// emela cannot read it.
    #[test]
    fn host_files_match_lib_dir() {
        let lib = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("lib");
        let mut on_disk = Vec::new();
        let mut dirs = vec![lib.clone()];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "mjs" || e == "c") {
                    let rel = path.strip_prefix(&lib).unwrap();
                    on_disk.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        on_disk.sort();
        let mut listed: Vec<String> = bundled_files().iter().map(|(p, _)| p.to_string()).collect();
        listed.sort();
        assert_eq!(listed, on_disk, "lib/ host files and HOST_FILES disagree");
    }

    #[test]
    fn platform_effects_are_bundled() {
        let modules: Vec<_> = core_sources().iter().map(|(m, _)| *m).collect();
        for module in [
            "Std.Console",
            "Std.Random",
            "Std.Env",
            "Std.Clock",
            "Std.FileSystem",
            "Std.Path",
            "Std.State",
            "Std.Platform",
        ] {
            assert!(modules.contains(&module), "{module} is not bundled");
        }
        assert!(core_source("Option").unwrap().contains("pub fn or_fail"));
        for (module, handlers) in [
            ("Std.Console", &["SystemConsole", "TestConsole"][..]),
            ("Std.Clock", &["SystemClock", "TestClock"]),
            ("Std.Random", &["SystemRandom"]),
            ("Std.Env", &["SystemEnv"]),
            ("Std.FileSystem", &["SystemFileSystem", "TestFileSystem"]),
        ] {
            for h in handlers {
                assert!(
                    core_source(module)
                        .unwrap()
                        .contains(&format!("pub handler {h}: ")),
                    "{module} has no {h}"
                );
            }
        }
        assert!(
            core_source("Std.Platform")
                .unwrap()
                .contains("pub layer Platform")
        );
        assert_eq!(lib_dir("Std.Clock"), "std");
        assert_eq!(lib_dir("List"), "core");
    }

    #[test]
    fn lookup_intrinsic_maps_prelude_to_unqualified() {
        assert_eq!(
            Builtin::lookup_intrinsic(PRELUDE, "panic"),
            Some(Builtin::Panic)
        );
        assert_eq!(
            Builtin::lookup_intrinsic("String", "length"),
            Some(Builtin::StringLength)
        );
        assert_eq!(Builtin::lookup_intrinsic("String", "panic"), None);
        assert_eq!(Builtin::lookup_intrinsic(PRELUDE, "length"), None);
        assert_eq!(
            core_source("List"),
            Some(include_str!("../lib/core/list.emel"))
        );
        assert_eq!(core_source("Map"), None);
    }
}
