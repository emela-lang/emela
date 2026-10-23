//! `Std.FileSystem` on the WASI 0.3 host: the preopened work directory, the stream and future
//! handshakes behind read/write/append/list, and the `error-code` classification. The test
//! program runs with the temporary directory preopened as the guest's work directory, which is
//! what `wasm_examples` does for every example.

mod common;

use std::path::{Path, PathBuf};

use common::*;

/// Runs `src` in a fresh directory (prepopulated with `files`), with `--dir` pointing at it.
fn run_fs(name: &str, files: &[(&str, &[u8])], src: &str) -> Option<Run> {
    run_fs_prepared(
        name,
        |dir| {
            for (path, contents) in files {
                let path = dir.join(path);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).unwrap();
                }
                std::fs::write(path, contents).unwrap();
            }
        },
        src,
    )
    .map(|(run, _)| run)
}

/// Like [`run_fs`], but `setup` prepares the directory (symbolic links, an absolute path) and
/// the test gets the directory back for host-side checks. The directory is absolute, so a
/// guest string built from it is an absolute path.
fn run_fs_prepared(name: &str, setup: impl FnOnce(&Path), src: &str) -> Option<(Run, PathBuf)> {
    wasmtime()?;
    let bytes = compile_checked(src);
    let dir = std::path::absolute(out_dir(name)).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    setup(&dir);
    let run = run_bytes_with_command(&dir, &bytes, |command| {
        command.arg("--dir").arg(&dir);
    });
    Some((run, dir))
}

/// The imports every test program in this file starts with.
const FS_IMPORTS: &str = "import Std.FileSystem.{FileSystem, SystemFileSystem, FileNotFound, FileAccessDenied, IsADirectory, InvalidText, NotADirectory, DirectoryNotEmpty, FileSystemFailure}";

/// An Emela helper: the name of the error `body` failed with, or `"ok"`.
const ERROR_NAME: &str = r#"fn error_name[E](body: fn() -> () fails E use { FileSystem }) -> String fails E use { FileSystem } {
  {
    body()
    "ok"
  } escape {
    FileNotFound(..)      -> "FileNotFound"
    FileAccessDenied(..)  -> "FileAccessDenied"
    IsADirectory(..)      -> "IsADirectory"
    InvalidText(..)       -> "InvalidText"
    NotADirectory(..)     -> "NotADirectory"
    DirectoryNotEmpty(..) -> "DirectoryNotEmpty"
    FileSystemFailure(..) -> "FileSystemFailure"
  }
}"#;

/// `path` as an Emela string literal.
fn emela_path(path: &Path) -> String {
    let text = path.to_str().expect("the path is UTF-8");
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '#' => out.push_str("\\#"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[test]
fn writes_reads_and_appends() {
    let src = "import Std.FileSystem.{FileSystem, SystemFileSystem}\n\nfn main() {\n  with SystemFileSystem {\n    fs = use FileSystem\n    fs.write_text(\"a.txt\", \"héllo 👍\")\n    fs.append_text(\"a.txt\", \"!\")\n    fs.append_text(\"b.txt\", \"new\")\n    dbg(fs.read_text(\"a.txt\"))\n    dbg(fs.read_text(\"b.txt\"))\n    fs.write_text(\"b.txt\", \"replaced\")\n    dbg(fs.read_text(\"b.txt\"))\n  }\n}\n";
    let Some(run) = run_fs("fs-write-read", &[], src) else {
        return;
    };
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stderr, "\"héllo 👍!\"\n\"new\"\n\"replaced\"\n");
}

#[test]
fn directories() {
    let src = "import Std.FileSystem.{FileSystem, SystemFileSystem}\n\nfn main() {\n  with SystemFileSystem {\n    fs = use FileSystem\n    fs.create_directory(\"d/x/y\")\n    fs.create_directory(\"d/x/y\")\n    fs.write_text(\"d/b.txt\", \"\")\n    fs.write_text(\"d/a.txt\", \"\")\n    dbg(fs.list_directory(\"d\"))\n    dbg(fs.exists(\"d/x\"))\n    dbg(fs.is_directory(\"d/x\"))\n    dbg(fs.is_directory(\"d/a.txt\"))\n    dbg(fs.exists(\"d/missing\"))\n    dbg(fs.is_directory(\"d/missing\"))\n    fs.remove(\"d/x/y\")\n    fs.remove(\"d/a.txt\")\n    dbg(fs.list_directory(\"d\"))\n  }\n}\n";
    let Some(run) = run_fs("fs-dirs", &[], src) else {
        return;
    };
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        run.stderr,
        "[\"a.txt\", \"b.txt\", \"x\"]\nTrue\nTrue\nFalse\nFalse\nFalse\n[\"b.txt\", \"x\"]\n"
    );
}

#[test]
fn errors_are_classified_like_js_and_c() {
    let body = r#"    fs.create_directory("e/sub")
    fs.write_text("e/f.txt", "f")
    dbg(outcome(fn() { _ = fs.read_text("e/missing.txt") }))
    dbg(outcome(fn() { _ = fs.read_text("e/sub") }))
    dbg(outcome(fn() { _ = fs.read_text("e/f.txt/x") }))
    dbg(outcome(fn() { fs.write_text("e/nowhere/x.txt", "x") }))
    dbg(outcome(fn() { fs.write_text("e/sub", "x") }))
    dbg(outcome(fn() { fs.append_text("e/nowhere/x.txt", "x") }))
    dbg(outcome(fn() { _ = fs.list_directory("e/missing") }))
    dbg(outcome(fn() { _ = fs.list_directory("e/f.txt") }))
    dbg(outcome(fn() { fs.create_directory("e/f.txt") }))
    dbg(outcome(fn() { fs.create_directory("e/f.txt/x") }))
    dbg(outcome(fn() { fs.remove("e/missing") }))
    dbg(outcome(fn() { fs.remove("e") }))"#;
    let src = format!(
        "import Std.FileSystem.{{FileSystem, SystemFileSystem, FileNotFound, IsADirectory, NotADirectory, DirectoryNotEmpty}}\n\nfn outcome[E](body: fn() -> () fails E use {{ FileSystem }}) -> String fails E use {{ FileSystem }} {{\n  {{\n    body()\n    \"ok\"\n  }} escape {{\n    FileNotFound(..)      -> \"FileNotFound\"\n    IsADirectory(..)      -> \"IsADirectory\"\n    NotADirectory(..)     -> \"NotADirectory\"\n    DirectoryNotEmpty(..) -> \"DirectoryNotEmpty\"\n  }}\n}}\n\nfn main() {{\n  with SystemFileSystem {{\n    fs = use FileSystem\n{body}\n  }}\n}}\n"
    );
    let Some(run) = run_fs("fs-errors", &[], &src) else {
        return;
    };
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        run.stderr,
        "\"FileNotFound\"\n\"IsADirectory\"\n\"FileNotFound\"\n\"FileNotFound\"\n\
         \"IsADirectory\"\n\"FileNotFound\"\n\"FileNotFound\"\n\"NotADirectory\"\n\
         \"NotADirectory\"\n\"NotADirectory\"\n\"FileNotFound\"\n\"DirectoryNotEmpty\"\n"
    );
}

#[test]
fn invalid_text_and_bom() {
    // `invalid.bin` is not UTF-8; `bom.txt` starts with a UTF-8 BOM, which is kept in the text.
    let body = r#"    dbg(outcome(fn() { _ = fs.read_text("invalid.bin") }))
    dbg(String.code_points(fs.read_text("bom.txt")))"#;
    let src = format!(
        "import Std.FileSystem.{{FileSystem, SystemFileSystem, InvalidText}}\n\nfn outcome[E](body: fn() -> () fails E use {{ FileSystem }}) -> String fails E use {{ FileSystem }} {{\n  {{\n    body()\n    \"ok\"\n  }} escape {{\n    InvalidText(..) -> \"InvalidText\"\n  }}\n}}\n\nfn main() {{\n  with SystemFileSystem {{\n    fs = use FileSystem\n{body}\n  }}\n}}\n"
    );
    let Some(run) = run_fs(
        "fs-invalid",
        &[
            ("invalid.bin", &[0x66, 0xff, 0x6f]),
            ("bom.txt", "\u{feff}x".as_bytes()),
        ],
        &src,
    ) else {
        return;
    };
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stderr, "\"InvalidText\"\n[65279, 120]\n");
}

/// The accepted WASM sandbox rejects absolute paths even when they name a file inside the
/// preopen. `exists` and `is_directory` return false. This pins the target difference
/// documented in NOTES.md.
#[cfg(unix)]
#[test]
fn absolute_paths_are_denied() {
    let name = "fs-absolute";
    let dir = std::path::absolute(out_dir(name)).unwrap();
    let src = format!(
        "{FS_IMPORTS}\n\n{ERROR_NAME}\n\nfn main() {{\n  with SystemFileSystem {{\n    fs = use FileSystem\n    dbg(error_name(fn() {{ _ = fs.read_text({abs_secret}) }}))\n    dbg((use FileSystem).exists({abs_secret}))\n    dbg((use FileSystem).is_directory({abs_dir}))\n  }}\n}}\n",
        abs_secret = emela_path(&dir.join("secret.txt")),
        abs_dir = emela_path(&dir),
    );
    let Some((run, _)) = run_fs_prepared(
        name,
        |dir| std::fs::write(dir.join("secret.txt"), "secret").unwrap(),
        &src,
    ) else {
        return;
    };
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stderr, "\"FileAccessDenied\"\nFalse\nFalse\n");
}

/// The accepted WASM sandbox: a `..` step that leaves the preopen reaches nothing, and a write
/// through it creates no file. `remove("..")` is denied too.
#[test]
fn dot_dot_cannot_leave_the_preopen() {
    let name = "fs-escape";
    let dir = std::path::absolute(out_dir(name)).unwrap();
    let parent = dir.parent().unwrap();
    let escape_read = parent.join(format!("{name}-outside.txt"));
    let escape_write = parent.join(format!("{name}-written.txt"));
    let _ = std::fs::remove_file(&escape_read);
    let _ = std::fs::remove_file(&escape_write);

    let src = format!(
        "{FS_IMPORTS}\n\n{ERROR_NAME}\n\nfn main() {{\n  with SystemFileSystem {{\n    fs = use FileSystem\n    dbg(error_name(fn() {{ _ = fs.read_text(\"../{name}-outside.txt\") }}))\n    dbg(error_name(fn() {{ fs.write_text(\"../{name}-written.txt\", \"x\") }}))\n    dbg(error_name(fn() {{ fs.remove(\"..\") }}))\n  }}\n}}\n"
    );
    let Some((run, _)) = run_fs_prepared(
        name,
        |_| std::fs::write(&escape_read, "outside").unwrap(),
        &src,
    ) else {
        return;
    };
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        run.stderr,
        "\"FileAccessDenied\"\n\"FileAccessDenied\"\n\"FileAccessDenied\"\n"
    );
    assert!(
        !escape_write.exists(),
        "the guest wrote outside the preopen"
    );
    let _ = std::fs::remove_file(&escape_read);
    let _ = std::fs::remove_file(&escape_write);
}

/// A symbolic link whose target is outside the preopen cannot be read, written or listed, and
/// nothing is created on the other side. `remove` deletes the link inside the preopen and
/// leaves its target alone.
#[cfg(unix)]
#[test]
fn a_symlink_out_of_the_preopen_is_denied() {
    use std::os::unix::fs::symlink;

    let name = "fs-symlink-escape";
    let dir = std::path::absolute(out_dir(name)).unwrap();
    let outside = dir.with_file_name(format!("{name}-outside"));
    let _ = std::fs::remove_dir_all(&outside);
    let src = format!(
        "{FS_IMPORTS}\n\n{ERROR_NAME}\n\nfn main() {{\n  with SystemFileSystem {{\n    fs = use FileSystem\n    dbg(error_name(fn() {{ _ = fs.read_text(\"escape/secret.txt\") }}))\n    dbg(error_name(fn() {{ fs.write_text(\"escape/pwn.txt\", \"x\") }}))\n    dbg(error_name(fn() {{ _ = fs.list_directory(\"escape\") }}))\n    dbg((use FileSystem).exists(\"escape/secret.txt\"))\n    dbg(error_name(fn() {{ fs.remove(\"escape\") }}))\n  }}\n}}\n"
    );
    let Some((run, dir)) = run_fs_prepared(
        name,
        |dir| {
            std::fs::create_dir_all(&outside).unwrap();
            std::fs::write(outside.join("secret.txt"), "outside").unwrap();
            symlink(&outside, dir.join("escape")).unwrap();
        },
        &src,
    ) else {
        return;
    };
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        run.stderr,
        "\"FileAccessDenied\"\n\"FileAccessDenied\"\n\"FileAccessDenied\"\nFalse\n\"ok\"\n"
    );
    assert!(
        !outside.join("pwn.txt").exists(),
        "the guest wrote through a symbolic link out of the preopen"
    );
    assert!(outside.join("secret.txt").exists());
    assert!(
        dir.join("escape").symlink_metadata().is_err(),
        "the symbolic link was not removed"
    );
    let _ = std::fs::remove_dir_all(&outside);
}

/// `remove` removes a final symbolic link itself, as on JS and C: a link to a directory, a
/// dangling link and a link to a file are all unlinked, and their targets stay.
#[cfg(unix)]
#[test]
fn removing_a_symlink_removes_the_link() {
    use std::os::unix::fs::symlink;

    let src = format!(
        "{FS_IMPORTS}\n\n{ERROR_NAME}\n\nfn main() {{\n  with SystemFileSystem {{\n    fs = use FileSystem\n    dbg(error_name(fn() {{ fs.remove(\"linkdir\") }}))\n    dbg(error_name(fn() {{ fs.remove(\"dangling\") }}))\n    dbg(error_name(fn() {{ fs.remove(\"linkfile\") }}))\n  }}\n}}\n"
    );
    let Some((run, dir)) = run_fs_prepared(
        "fs-symlink-remove",
        |dir| {
            std::fs::create_dir_all(dir.join("sub")).unwrap();
            std::fs::write(dir.join("sub/file.txt"), "x").unwrap();
            symlink("sub", dir.join("linkdir")).unwrap();
            symlink("missing", dir.join("dangling")).unwrap();
            symlink("sub/file.txt", dir.join("linkfile")).unwrap();
        },
        &src,
    ) else {
        return;
    };
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stderr, "\"ok\"\n\"ok\"\n\"ok\"\n");
    for link in ["linkdir", "dangling", "linkfile"] {
        assert!(
            dir.join(link).symlink_metadata().is_err(),
            "the symbolic link {link} was not removed"
        );
    }
    assert!(
        dir.join("sub/file.txt").exists(),
        "removing a symbolic link removed its target"
    );
}

#[test]
fn file_system_is_only_imported_when_used() {
    let imports = component_imports(&compile_checked(
        "import Std.FileSystem.{FileSystem, SystemFileSystem}\n\nfn main() {\n  with SystemFileSystem {\n    (use FileSystem).exists(\"x\")\n  }\n}\n",
    ));
    assert!(
        imports.iter().any(|i| i == "wasi:filesystem/types@0.3.0"),
        "{imports:?}"
    );
    assert!(
        imports
            .iter()
            .any(|i| i == "wasi:filesystem/preopens@0.3.0"),
        "{imports:?}"
    );
}

/// The interface names a component imports (the same probe as `tests/hosts.rs`).
fn component_imports(bytes: &[u8]) -> Vec<String> {
    use wasmparser::{Parser, Payload};
    let mut imports = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        if let Payload::ComponentImportSection(section) = payload.unwrap() {
            for import in section {
                imports.push(import.unwrap().name.full_name().into_owned());
            }
        }
    }
    imports
}

#[test]
fn a_large_text_grows_the_read_buffer() {
    // One 50 KB file exercises the growing buffer and several stream reads.
    let src = "import Std.FileSystem.{FileSystem, SystemFileSystem}\n\nfn main() {\n  with SystemFileSystem {\n    fs = use FileSystem\n    text = String.join(List.map(List.range(0, 5000), fn(_) { \"0123456789\" }), \"\")\n    fs.write_text(\"big.txt\", text)\n    dbg(String.length(fs.read_text(\"big.txt\")))\n    dbg(String.length(text))\n  }\n}\n";
    let Some(run) = run_fs("fs-large", &[], src) else {
        return;
    };
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stderr, "50000\n50000\n");
}

#[test]
fn a_long_listing_reads_several_chunks() {
    // 40 entries do not fit one `read-directory` chunk (16 records), so the loop reads again.
    let src = "import Std.FileSystem.{FileSystem, SystemFileSystem}\n\nfn main() {\n  with SystemFileSystem {\n    fs = use FileSystem\n    fs.create_directory(\"m\")\n    List.fold(List.range(0, 40), from: (), f: fn(_, i) { fs.write_text(\"m/#{i}.txt\", \"\") })\n    names = fs.list_directory(\"m\")\n    dbg(List.length(names))\n    dbg(List.all(names, fn(n) { String.ends_with(n, \".txt\") }))\n  }\n}\n";
    let Some(run) = run_fs("fs-many", &[], src) else {
        return;
    };
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stderr, "40\nTrue\n");
}
