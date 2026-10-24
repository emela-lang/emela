// crates/emela-driver/src/new.rs
use std::path::PathBuf;

use crate::diagnostic::Diagnostic;

/// The Pome `emela new` creates.
pub struct NewPome {
    pub dir: String,
    pub files: Vec<NewFile>,
}

pub struct NewFile {
    pub path: PathBuf,
    pub contents: String,
}

pub fn new(name: &str, lib: bool) -> Result<NewPome, Diagnostic> {
    let source = source_path(name);
    let last = match &source {
        Some(path) => {
            if path.split('/').any(str::is_empty) {
                return Err(Diagnostic::error(format!("`{path}` has an empty segment")));
            }
            path.rsplit('/')
                .next()
                .expect("split yields at least one segment")
        }
        None => name,
    };
    check_name(last)?;

    let mut files = vec![
        NewFile {
            path: PathBuf::from("Pome.toml"),
            contents: pome_toml(source.as_deref(), last),
        },
        NewFile {
            path: PathBuf::from(".gitignore"),
            contents: "target/\n".to_owned(),
        },
    ];
    files.push(if lib {
        NewFile {
            path: PathBuf::from("src/lib.emel"),
            contents: LIB_EMEL.to_owned(),
        }
    } else {
        NewFile {
            path: PathBuf::from("src/main.emel"),
            contents: MAIN_EMEL.to_owned(),
        }
    });
    Ok(NewPome {
        dir: last.to_owned(),
        files,
    })
}

const MAIN_EMEL: &str = "\
import Std.Console.{Console, SystemConsole}

fn main() {
  with SystemConsole {
    console = use Console
    console.print_line(\"Hello, world!\")
  }
}
";

const LIB_EMEL: &str = "\
## Adds two numbers.
pub fn add(a: Int, b: Int) -> Int {
  a + b
}

@test
fn add_works() {
  assert add(1, 2) == 3
}
";

/// `Pome.toml` with `name`, `version` and `emela` (spec 4.7). A bare name gets a placeholder
/// source path and a comment asking to replace it.
fn pome_toml(source: Option<&str>, last: &str) -> String {
    let name = match source {
        Some(path) => format!("name = \"{path}\"\n"),
        None => format!(
            "# Replace with the source path of your repository (e.g. github.com/alice/{last}).\n\
             name = \"github.com/your-name/{last}\"\n"
        ),
    };
    format!(
        "[pome]\n{name}version = \"0.1.0\"\nemela = \"{}\"\n",
        env!("CARGO_PKG_VERSION")
    )
}

/// The checks only `emela new` makes on the last segment; a hand-written `Pome.toml` may still
/// use any name.
fn check_name(last: &str) -> Result<(), Diagnostic> {
    let snake = last.replace('-', "_");
    let root = emela_resolve::to_type_name(&snake).map_err(|error| {
        Diagnostic::error(format!("`{last}` cannot be a Pome name: {error}"))
            .with_help("use lowercase letters, digits, `_` and `-`, starting with a letter")
    })?;
    let mut core = emela_core::core_sources()
        .iter()
        .map(|(module, _)| *module)
        .filter(|module| !module.contains('.'));
    if root == emela_core::STD || core.any(|module| root == module) {
        return Err(Diagnostic::error(format!(
            "`{last}` cannot be a Pome name: its root `{root}` is a core or std module"
        )));
    }
    if snake.starts_with("emela_") {
        return Err(Diagnostic::error(format!(
            "`{last}` cannot be a Pome name: names starting with `emela-` or `emela_` are reserved"
        )));
    }
    Ok(())
}

/// The source path the input names (spec 4.7), or `None` for a bare name.
///
/// - `hello` (neither `/` nor `:`) is a bare name. The source path is not guessed.
/// - `github:alice/json-codec` becomes `github.com/alice/json-codec`.
/// - `https://` in front and `.git` at the end are removed.
fn source_path(name: &str) -> Option<String> {
    if !name.contains('/') && !name.contains(':') {
        return None;
    }
    let path = name.strip_prefix("https://").unwrap_or(name);
    let path = path.strip_suffix(".git").unwrap_or(path);
    Some(match path.strip_prefix("github:") {
        Some(rest) => format!("github.com/{rest}"),
        None => path.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn created(name: &str, lib: bool) -> NewPome {
        new(name, lib)
            .ok()
            .unwrap_or_else(|| panic!("`{name}` should be accepted"))
    }

    fn contents<'a>(pome: &'a NewPome, path: &str) -> Option<&'a str> {
        pome.files
            .iter()
            .find(|file| file.path == Path::new(path))
            .map(|file| file.contents.as_str())
    }

    #[test]
    fn inputs_become_name_and_directory() {
        let cases = [
            ("hello", "github.com/your-name/hello", "hello"),
            (
                "github.com/alice/json-codec",
                "github.com/alice/json-codec",
                "json-codec",
            ),
            (
                "github:alice/json-codec",
                "github.com/alice/json-codec",
                "json-codec",
            ),
            (
                "https://github.com/alice/json-codec.git",
                "github.com/alice/json-codec",
                "json-codec",
            ),
        ];
        for (input, name, dir) in cases {
            let pome = created(input, false);
            assert_eq!(pome.dir, dir, "{input}");
            let toml = contents(&pome, "Pome.toml").unwrap();
            assert!(
                toml.contains(&format!("name = \"{name}\"\n")),
                "{input}: {toml}"
            );
        }
    }

    #[test]
    fn only_a_bare_name_gets_the_placeholder_comment() {
        let bare = created("hello", false);
        assert!(
            contents(&bare, "Pome.toml")
                .unwrap()
                .contains("# Replace with")
        );
        let path = created("github:alice/hello", false);
        assert!(!contents(&path, "Pome.toml").unwrap().contains('#'));
    }

    #[test]
    fn rejects_names() {
        for input in [
            "Hello",
            "a",
            "x_y",
            "日本語",
            "1abc",
            "std",
            "prelude",
            "list",
            "string",
            "int64",
            "emela-json",
            "emela_json",
            "github.com//x",
            "github:alice/",
        ] {
            assert!(new(input, false).is_err(), "`{input}` should be rejected");
        }
    }

    #[test]
    fn accepts_digits_after_separators() {
        assert_eq!(created("json-codec", false).dir, "json-codec");
        assert_eq!(created("http_2", false).dir, "http_2");
    }

    #[test]
    fn application_and_library_files() {
        let paths = |pome: &NewPome| {
            let mut paths: Vec<_> = pome.files.iter().map(|file| file.path.clone()).collect();
            paths.sort();
            paths
        };
        assert_eq!(
            paths(&created("hello", false)),
            [".gitignore", "Pome.toml", "src/main.emel"].map(PathBuf::from)
        );
        assert_eq!(
            paths(&created("hello", true)),
            [".gitignore", "Pome.toml", "src/lib.emel"].map(PathBuf::from)
        );
        assert_eq!(
            contents(&created("hello", false), ".gitignore"),
            Some("target/\n")
        );
    }

    #[test]
    fn emela_version_is_the_compiler_version() {
        let toml = contents(&created("hello", false), "Pome.toml")
            .unwrap()
            .to_owned();
        let expected = format!("emela = \"{}\"\n", env!("CARGO_PKG_VERSION"));
        assert!(toml.contains(&expected), "{toml}");
    }
}
