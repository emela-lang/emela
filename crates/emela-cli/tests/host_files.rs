//! Files `@external` points to are copied into the JS output and imported relatively (spec 12.1),
//! so the output runs after being moved, for the project's own files and a dependency's.

use std::path::Path;
use std::process::Command;

fn write(dir: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

fn emela(dir: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_emela"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .env("EMELA_POMEPROXY", "off")
        .env("EMELA_POME_CACHE", dir.join(".cache"))
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into(),
        String::from_utf8_lossy(&out.stderr).into(),
    )
}

// Runs the built output from another directory, to show it does not depend on the sources.
fn run_moved(project: &Path) -> String {
    let moved = tempfile::tempdir().unwrap();
    let target = moved.path().join("out");
    std::fs::rename(project.join("target/emela/js"), &target).unwrap();
    // The entry exports `main`; `emela run` calls it the same way.
    let call = "const { pathToFileURL } = await import(\"node:url\"); \
                const m = await import(pathToFileURL(process.argv[1]).href); m.main();";
    let out = Command::new("node")
        .args(["--input-type=module", "-e", call])
        .arg(target.join("main.mjs"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into()
}

const MAIN: &str = "import Std.Console.{Console, SystemConsole}\n\n@external(js, \"./ffi.mjs\", \"greet\")\nfn greet(name: String) -> String use {}\n\nfn main() {\n  with SystemConsole {\n    console = use Console\n    console.print_line(greet(\"Pome\"))\n  }\n}\n";

#[test]
fn copies_the_projects_host_files() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        &[
            ("Pome.toml", ""),
            ("src/main.emel", MAIN),
            (
                "src/ffi.mjs",
                "import { suffix } from \"./lib/helper.mjs\";\nexport const greet = (n) => `hello, ${n}${suffix}`;\n",
            ),
            ("src/lib/helper.mjs", "export const suffix = \"!\";\n"),
        ],
    );
    let (code, _, stderr) = emela(dir.path(), &["build"]);
    assert_eq!(code, Some(0), "{stderr}");
    let out = dir.path().join("target/emela/js");
    let main = std::fs::read_to_string(out.join("main.mjs")).unwrap();
    assert!(main.contains("from \"./emela_project/ffi.mjs\""), "{main}");
    assert!(!main.contains("file://"), "{main}");
    assert!(out.join("emela_project/lib/helper.mjs").is_file());
    assert!(!out.join("emela_project/main.emel").exists());
    assert_eq!(run_moved(dir.path()), "hello, Pome!\n");
}

#[test]
fn copies_a_dependencys_host_files() {
    let dir = tempfile::tempdir().unwrap();
    let app = "import Shout\nimport Std.Console.{Console, SystemConsole}\n\nfn main() {\n  with SystemConsole {\n    console = use Console\n    console.print_line(Shout.loud(\"pome\"))\n  }\n}\n";
    write(
        dir.path(),
        &[
            (
                "app/Pome.toml",
                "[pome]\nname = \"github.com/me/app\"\nversion = \"0.1.0\"\nemela = \"0.21.0\"\n\n[dependencies]\n\"github.com/acme/shout\" = \"v0.1.0\"\n\n[replace]\n\"github.com/acme/shout\" = { path = \"../shout\" }\n\n[external]\nallow = [\"github.com/acme/shout\"]\n",
            ),
            ("app/src/main.emel", app),
            (
                "shout/Pome.toml",
                "[pome]\nname = \"github.com/acme/shout\"\nversion = \"0.1.0\"\nemela = \"0.21.0\"\ntargets = [\"js\"]\n",
            ),
            (
                "shout/src/lib.emel",
                "@external(js, \"./host.mjs\", \"shout\")\nfn shout(text: String) -> String use {}\n\npub fn loud(text: String) -> String { shout(text) }\n",
            ),
            (
                "shout/src/host.mjs",
                "export const shout = (t) => t.toUpperCase();\n",
            ),
        ],
    );
    let app = dir.path().join("app");
    let (code, _, stderr) = emela(&app, &["pome", "install"]);
    assert_eq!(code, Some(0), "{stderr}");
    let (code, _, stderr) = emela(&app, &["build"]);
    assert_eq!(code, Some(0), "{stderr}");
    let out = app.join("target/emela/js");
    let main = std::fs::read_to_string(out.join("main.mjs")).unwrap();
    assert!(
        main.contains("from \"./emela_pomes/github.com/acme/shout@replace/host.mjs\""),
        "{main}"
    );
    assert_eq!(run_moved(&app), "POME\n");
}
