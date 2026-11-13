//! The JS host: the command that runs the JS output, chosen from the environment, and the check
//! of its version before a run (`[cmd.build]`, `[target]`).

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::code;
use crate::diagnostic::Diagnostic;
use crate::doctor::Version;

/// The environment variable that names the JS host (`[cmd.build]`).
pub const JS_HOST_ENV: &str = "EMELA_JS_HOST";
/// The deprecated environment variable that named the JS host before [`JS_HOST_ENV`]. It is read
/// after [`JS_HOST_ENV`], and reading it reports W0902 (`[cmd.build]`).
pub const NODE_ENV: &str = "EMELA_NODE";

/// The JS host chosen from the environment (`[cmd.build]`).
#[derive(Debug, Clone)]
pub struct JsHost {
    /// The command that runs the JS output.
    pub program: PathBuf,
    /// W0902 when `EMELA_NODE` was read. The caller reports it when it uses the JS host.
    pub warning: Option<Diagnostic>,
}

/// `EMELA_JS_HOST` if it is set and not empty, otherwise `EMELA_NODE` if it is set and not
/// empty, otherwise `node` (`[cmd.build]`).
///
/// `EMELA_NODE` counts as read when it is set and not empty, also when `EMELA_JS_HOST` takes
/// precedence. An empty value chooses nothing, the same as an unset one, so it is not reported.
pub(crate) fn js_host_in(env: &dyn Fn(&str) -> Option<OsString>) -> JsHost {
    let value = |key| env(key).filter(|v: &OsString| !v.is_empty());
    let node = value(NODE_ENV);
    let warning = node.is_some().then(|| {
        Diagnostic::warning(format!("{NODE_ENV} is deprecated; use {JS_HOST_ENV}"))
            .with_code(code::NODE_ENV_DEPRECATED)
    });
    let program = value(JS_HOST_ENV)
        .or(node)
        .map_or_else(|| PathBuf::from("node"), PathBuf::from);
    JsHost { program, warning }
}

/// The JS host chosen from the environment of this process ([`js_host_in`]).
pub fn js_host() -> JsHost {
    js_host_in(&|key| std::env::var_os(key))
}

/// The command of the JS host chosen from the environment of this process, without the warning.
pub fn js_host_program() -> PathBuf {
    js_host().program
}

/// A JS host that the JS target supports (`[target]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Node,
    Bun,
}

impl Kind {
    /// The name in messages (`Node`, `Bun`).
    pub(crate) fn name(self) -> &'static str {
        match self {
            Kind::Node => "Node",
            Kind::Bun => "Bun",
        }
    }

    /// The word that [`PROBE`] writes before the version.
    pub(crate) fn word(self) -> &'static str {
        match self {
            Kind::Node => "node",
            Kind::Bun => "bun",
        }
    }

    /// The oldest version that `[target]` supports, as major, minor and patch, and as it is
    /// written in messages.
    fn minimum(self) -> ((u32, u32, u32), &'static str) {
        match self {
            Kind::Node => ((22, 0, 0), "22"),
            Kind::Bun => ((1, 4, 3), "1.4.3"),
        }
    }

    /// `Node 22`, `Bun 1.4.3`.
    pub(crate) fn minimum_text(self) -> String {
        format!("{} {}", self.name(), self.minimum().1)
    }
}

/// What every supported JS host needs, for messages that do not know which host it is.
pub(crate) const NEEDS: &str = "Node 22 or later or Bun 1.4.3 or later";

/// The module that the JS host runs with `-e` to report itself (`[cmd.build]`). Bun defines
/// `process.versions.node` as well, so `bun` is looked at first.
pub(crate) const PROBE: &str = "const v = process.versions;\nprocess.stdout.write(v.bun === undefined ? `node ${v.node}\\n` : `bun ${v.bun}\\n`);\n";

/// The host and version that [`PROBE`] wrote.
pub(crate) struct Probed {
    pub(crate) kind: Kind,
    pub(crate) version: Version,
}

impl Probed {
    /// Older than the minimum of its host. A missing minor or patch counts as 0.
    pub(crate) fn too_old(&self) -> bool {
        let (minimum, _) = self.kind.minimum();
        let v = &self.version;
        match v.major {
            Some(major) => (major, v.minor.unwrap_or(0), v.patch.unwrap_or(0)) < minimum,
            None => false,
        }
    }

    /// `Node 20.11.1`.
    pub(crate) fn describe(&self) -> String {
        format!("{} {}", self.kind.name(), self.version.text)
    }
}

/// Reads `node <version>` or `bun <version>` from the output of [`PROBE`]. The first line in that
/// form counts, so that a wrapper set in `EMELA_JS_HOST` may write other lines.
pub(crate) fn parse(out: &str) -> Option<Probed> {
    out.lines().find_map(|line| {
        let (word, version) = line.trim().split_once(' ')?;
        let kind = [Kind::Node, Kind::Bun]
            .into_iter()
            .find(|kind| kind.word() == word)?;
        Some(Probed {
            kind,
            version: crate::doctor::number(version.trim())?,
        })
    })
}

/// Starts `program` with `--input-type=module -e` and [`PROBE`], and reads what it wrote to
/// standard output. `Ok(None)` when it started but wrote nothing in the form of [`parse`].
pub(crate) fn probe(program: &Path) -> io::Result<Option<Probed>> {
    let out = Command::new(program)
        .arg("--input-type=module")
        .arg("-e")
        .arg(PROBE)
        .stdin(Stdio::null())
        .output()?;
    Ok(parse(&String::from_utf8_lossy(&out.stdout)))
}

/// E0904 when the JS host is older than the minimum of its host (`[cmd.build]`). Nothing is
/// reported when the version cannot be read: a host that does not start is reported by the run
/// itself.
pub(crate) fn check_version(program: &Path) -> Result<(), Diagnostic> {
    match probe(program) {
        Ok(Some(probed)) if probed.too_old() => Err(Diagnostic::error(format!(
            "`{}` is {}, but target `js` needs {} or later",
            program.display(),
            probed.describe(),
            probed.kind.minimum_text(),
        ))
        .with_code(code::NODE_UNAVAILABLE)
        .with_note(too_old_help())),
        _ => Ok(()),
    }
}

/// A diagnostic about failing to start the JS host.
pub(crate) fn spawn_error(program: &Path, err: &io::Error) -> Diagnostic {
    match err.kind() {
        io::ErrorKind::TimedOut => Diagnostic::error(format!("`{}` {err}", program.display())),
        io::ErrorKind::NotFound => Diagnostic::error(format!("`{}` not found", program.display()))
            .with_code(code::NODE_UNAVAILABLE)
            .with_note(missing_help()),
        _ => Diagnostic::error(format!("cannot start `{}`: {err}", program.display()))
            .with_code(code::NODE_UNAVAILABLE),
    }
}

/// The note of E0904 when the JS host is not found.
pub(crate) fn missing_help() -> String {
    format!(
        "install Node.js 22 or later or Bun 1.4.3 or later and add it to PATH, or set {JS_HOST_ENV} to the JS host"
    )
}

/// The note of E0904 when the JS host is too old (`[cmd.build]`).
pub(crate) fn too_old_help() -> String {
    format!(
        "install Node.js 22 or later or Bun 1.4.3 or later, or set {JS_HOST_ENV} to a newer JS host"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let pairs: Vec<(String, OsString)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), OsString::from(v)))
            .collect();
        move |key| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    fn chosen(pairs: &[(&str, &str)]) -> (PathBuf, Option<(String, Option<String>)>) {
        let host = js_host_in(&vars(pairs));
        (
            host.program,
            host.warning.map(|w| (w.message, w.code.map(str::to_owned))),
        )
    }

    fn warned() -> Option<(String, Option<String>)> {
        Some((
            "EMELA_NODE is deprecated; use EMELA_JS_HOST".to_owned(),
            Some("W0902".to_owned()),
        ))
    }

    #[test]
    fn the_js_host_is_node_when_nothing_is_set() {
        assert_eq!(chosen(&[]), (PathBuf::from("node"), None));
        assert_eq!(
            chosen(&[("EMELA_JS_HOST", ""), ("EMELA_NODE", "")]),
            (PathBuf::from("node"), None)
        );
    }

    #[test]
    fn emela_js_host_names_the_js_host() {
        assert_eq!(
            chosen(&[("EMELA_JS_HOST", "bun")]),
            (PathBuf::from("bun"), None)
        );
        assert_eq!(
            chosen(&[("EMELA_JS_HOST", "/opt/bun/bin/bun")]),
            (PathBuf::from("/opt/bun/bin/bun"), None)
        );
    }

    #[test]
    fn emela_node_is_read_with_w0902() {
        assert_eq!(
            chosen(&[("EMELA_NODE", "/opt/node/bin/node")]),
            (PathBuf::from("/opt/node/bin/node"), warned())
        );
        // An empty `EMELA_JS_HOST` chooses nothing.
        assert_eq!(
            chosen(&[("EMELA_JS_HOST", ""), ("EMELA_NODE", "bun")]),
            (PathBuf::from("bun"), warned())
        );
    }

    #[test]
    fn emela_js_host_takes_precedence_and_emela_node_still_warns() {
        assert_eq!(
            chosen(&[("EMELA_JS_HOST", "bun"), ("EMELA_NODE", "node")]),
            (PathBuf::from("bun"), warned())
        );
    }

    /// The host, the version as written, its major and minor, and whether it is too old.
    type Parsed = (Kind, String, Option<u32>, Option<u32>, bool);

    fn parsed(out: &str) -> Option<Parsed> {
        parse(out).map(|p| {
            let too_old = p.too_old();
            (
                p.kind,
                p.version.text,
                p.version.major,
                p.version.minor,
                too_old,
            )
        })
    }

    #[test]
    fn reads_node_and_bun() {
        assert_eq!(
            parsed("node 24.11.0\n"),
            Some((Kind::Node, "24.11.0".into(), Some(24), Some(11), false))
        );
        assert_eq!(
            parsed("bun 1.4.3\n"),
            Some((Kind::Bun, "1.4.3".into(), Some(1), Some(4), false))
        );
        // A wrapper may write other lines first.
        assert_eq!(
            parsed("using a wrapper\r\nnode 22.0.0\r\n"),
            Some((Kind::Node, "22.0.0".into(), Some(22), Some(0), false))
        );
        assert!(parsed("").is_none());
        assert!(parsed("v24.11.0\n").is_none());
        assert!(parsed("deno 2.0.0\n").is_none());
        assert!(parsed("node\n").is_none());
    }

    #[test]
    fn each_host_is_compared_with_its_own_minimum() {
        // Node 22 or later (`[target]`).
        let too_old = |out| parsed(out).unwrap().4;
        assert!(too_old("node 21.7.3"));
        assert!(!too_old("node 22.0.0"));
        // Bun 1.4.3 or later, to the patch version; a Bun is not compared with the minimum of
        // Node.
        assert!(too_old("bun 1.2.23"));
        assert!(too_old("bun 1.3.14"));
        assert!(too_old("bun 1.4.2"));
        assert!(!too_old("bun 1.4.3"));
        assert!(!too_old("bun 1.4.10"));
        assert!(!too_old("bun 1.5.0"));
        assert!(!too_old("bun 2.0.0"));
        // A missing part counts as 0.
        assert!(too_old("bun 1"));
        assert!(too_old("bun 1.4"));
        assert!(!too_old("bun 1.5"));
    }

    #[test]
    fn the_probe_writes_the_host_and_its_version() {
        assert!(PROBE.contains("process.versions"));
        assert!(PROBE.contains("`bun ${v.bun}"));
        assert!(PROBE.contains("`node ${v.node}"));
    }
}

/// Tests that start fake JS hosts written to a temporary directory.
#[cfg(all(test, unix))]
mod process_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fake(dir: &Path, name: &str, script: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// A host that writes `line` when it is started as `[cmd.build]` says, and `wrong` otherwise.
    fn host(dir: &Path, line: &str) -> PathBuf {
        fake(
            dir,
            "host",
            &format!(
                "#!/bin/sh\nif [ \"$1\" = --input-type=module ] && [ \"$2\" = -e ] && [ $# = 3 ]; then echo '{line}'; else echo wrong; fi\n"
            ),
        )
    }

    #[test]
    fn the_probe_starts_the_host_with_the_module() {
        let dir = tempfile::tempdir().unwrap();
        let bun = host(dir.path(), "bun 1.4.3");
        let probed = probe(&bun).unwrap().unwrap();
        assert_eq!(
            (probed.kind, probed.version.text.as_str()),
            (Kind::Bun, "1.4.3")
        );
    }

    #[test]
    fn e0904_for_a_node_that_is_too_old() {
        let dir = tempfile::tempdir().unwrap();
        let node = host(dir.path(), "node 20.11.1");
        let err = check_version(&node).unwrap_err();
        assert_eq!(err.code, Some("E0904"));
        assert_eq!(
            err.message,
            format!(
                "`{}` is Node 20.11.1, but target `js` needs Node 22 or later",
                node.display()
            )
        );
        assert_eq!(
            err.notes,
            vec![
                "install Node.js 22 or later or Bun 1.4.3 or later, or set EMELA_JS_HOST to a newer JS host"
                    .to_owned()
            ]
        );
    }

    #[test]
    fn e0904_for_a_bun_that_is_too_old() {
        let dir = tempfile::tempdir().unwrap();
        let bun = host(dir.path(), "bun 1.4.2");
        let err = check_version(&bun).unwrap_err();
        assert_eq!(
            err.message,
            format!(
                "`{}` is Bun 1.4.2, but target `js` needs Bun 1.4.3 or later",
                bun.display()
            )
        );
    }

    #[test]
    fn a_host_that_is_new_enough_or_unknown_or_cannot_start_passes() {
        let dir = tempfile::tempdir().unwrap();
        for line in ["node 22.0.0", "bun 1.4.3", "bun 1.4.4", "hello"] {
            assert!(check_version(&host(dir.path(), line)).is_ok(), "{line}");
        }
        let broken = fake(dir.path(), "broken", "#!/nonexistent/sh\n");
        assert!(check_version(&broken).is_ok());
        assert!(check_version(&dir.path().join("missing")).is_ok());
    }
}
