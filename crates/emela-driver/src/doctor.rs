use std::ffi::OsString;
use std::fmt::Write;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::Target;
use emela_pome::fetch::config::{Config, Source};

pub(crate) struct Version {
    pub(crate) text: String,
    pub(crate) major: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Ok,
    Missing,
    TooOld,
    Failed,
}

impl Status {
    /// The word in the output, in the text and in `status` of the JSON.
    fn label(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Missing => "missing",
            Status::TooOld => "too-old",
            Status::Failed => "failed",
        }
    }
}

struct ProgramCheck {
    name: &'static str,
    status: Status,
    executable: PathBuf,
    version: Option<String>,
    message: Option<String>,
    help: Option<String>,
}

struct Spec {
    name: &'static str,
    who: &'static str,
    program: PathBuf,
    parse: fn(&str) -> Option<Version>,
    min_major: Option<u32>,
    needs: String,
    missing_help: Option<String>,
    too_old_help: Option<String>,
}

#[derive(Debug)]
struct PomeSettings {
    proxy: Vec<String>,
    sumdb: Option<String>,
    private: usize,
}

struct PomeReport {
    settings: Result<PomeSettings, String>,
    git: ProgramCheck,
    git_required: bool,
}

struct Report {
    targets: Vec<TargetReport>,
    pome: PomeReport,
}

struct TargetReport {
    target: Target,
    required: bool,
    programs: Vec<ProgramCheck>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoctorFormat {
    Text,
    Json,
}

pub fn doctor(
    required: Option<Target>,
    version: &str,
    format: DoctorFormat,
    color: bool,
) -> (String, u8) {
    let env = |key: &str| std::env::var_os(key);
    let report = report(required, &env);
    let output = match format {
        DoctorFormat::Text => render_text(&report, version, color),
        DoctorFormat::Json => render_json(&report, version),
    };
    (output, exit_code(&report))
}

fn number(word: &str) -> Option<Version> {
    let major = word.split('.').next().and_then(|s| s.parse::<u32>().ok())?;

    Some(Version {
        text: word.to_string(),
        major: Some(major),
    })
}

pub(crate) fn node_version(out: &str) -> Option<Version> {
    let word = out.trim();
    number(word.strip_prefix('v').unwrap_or(word))
}

pub(crate) fn wasmtime_version(out: &str) -> Option<Version> {
    let word = out.split_whitespace().nth(1)?;
    number(word)
}

fn git_version(out: &str) -> Option<Version> {
    let word = out.trim();
    number(
        word.strip_prefix("git version ")?
            .split_whitespace()
            .next()?,
    )
}

fn cc_version(out: &str) -> Option<Version> {
    let word = out.lines().find(|line| !line.trim().is_empty())?;
    Some(Version {
        text: word.to_string(),
        major: None,
    })
}

#[cfg(unix)]
fn find_executable(program: &Path, env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let is_executable = |p: &Path| {
        p.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };

    if program.components().count() > 1 {
        return is_executable(program)
            .then(|| std::path::absolute(program).ok())
            .flatten();
    }

    env("PATH").and_then(|path| {
        std::env::split_paths(&path).find_map(|dir| {
            let candidate = dir.join(program);
            is_executable(&candidate).then_some(candidate)
        })
    })
}

fn probe(
    exe: &Path,
    parse: fn(&str) -> Option<Version>,
) -> Result<Option<Version>, std::io::Error> {
    let out = Command::new(exe)
        .arg("--version")
        .stdin(Stdio::null())
        .output()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if let Some(version) = parse(&stdout) {
        return Ok(Some(version));
    }

    let stderr = String::from_utf8_lossy(&out.stderr);
    Ok(parse(&stderr))
}

fn status(probed: Option<&io::Result<Option<Version>>>, min_major: Option<u32>) -> Status {
    match probed {
        None => Status::Missing,
        Some(Err(_)) => Status::Failed,
        Some(Ok(Some(Version { major: Some(m), .. }))) if min_major.is_some_and(|min| *m < min) => {
            Status::TooOld
        }
        Some(Ok(_)) => Status::Ok,
    }
}

fn check(spec: &Spec, env: &dyn Fn(&str) -> Option<OsString>) -> ProgramCheck {
    let executable = find_executable(&spec.program, env);
    let probed = executable.as_ref().map(|exe| probe(exe, spec.parse));
    let status = status(probed.as_ref(), spec.min_major);
    let shown = executable.as_ref().unwrap_or(&spec.program);
    let version = probed
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .and_then(|v| v.as_ref())
        .map(|v| v.text.clone());
    let error = match &probed {
        Some(Err(e)) => Some(e.to_string()),
        _ => None,
    };
    let lead = format!(
        "{} needs {}, but `{}`",
        spec.who,
        spec.needs,
        shown.display()
    );

    let (message, help) = match status {
        Status::Ok => (None, None),
        Status::Missing => {
            // A name with a directory is not looked up on PATH (see `find_executable`).
            let why = if spec.program.components().count() > 1 {
                "is not an executable file"
            } else {
                "is not on PATH"
            };
            (Some(format!("{lead} {why}")), spec.missing_help.clone())
        }
        Status::TooOld => (
            Some(format!(
                "{lead} is {}",
                version.as_deref().unwrap_or_default()
            )),
            spec.too_old_help.clone(),
        ),
        Status::Failed => (
            Some(format!(
                "{lead} cannot be started: {}",
                error.unwrap_or_default()
            )),
            None,
        ),
    };

    ProgramCheck {
        name: spec.name,
        status,
        executable: shown.clone(),
        version,
        message,
        help,
    }
}

fn node_spec(env: &dyn Fn(&str) -> Option<OsString>) -> Spec {
    Spec {
        name: "node",
        who: "target `js`",
        program: crate::run::node_program_in(env),
        parse: node_version,
        min_major: Some(crate::run::NODE_MIN_MAJOR),
        needs: format!("node {} or later", crate::run::NODE_MIN_MAJOR),
        missing_help: Some(crate::run::NODE.missing_help()),
        too_old_help: Some(crate::run::NODE.too_old_help()),
    }
}

fn wasmtime_spec(env: &dyn Fn(&str) -> Option<OsString>) -> Spec {
    Spec {
        name: "wasmtime",
        who: "target `wasm`",
        program: crate::run::wasmtime_program_in(env),
        parse: wasmtime_version,
        min_major: Some(crate::run::WASMTIME_MIN_MAJOR),
        needs: format!("wasmtime {} or later", crate::run::WASMTIME_MIN_MAJOR),
        missing_help: Some(crate::run::WASMTIME.missing_help()),
        too_old_help: Some(crate::run::WASMTIME.too_old_help()),
    }
}

fn cc_spec(env: &dyn Fn(&str) -> Option<OsString>) -> Spec {
    Spec {
        name: "cc",
        who: "target `c`",
        program: crate::c::cc_command_in(env).0,
        parse: cc_version,
        min_major: None,
        needs: "a C compiler".into(),
        missing_help: Some(crate::c::cc_missing_help()),
        too_old_help: None,
    }
}

fn git_spec() -> Spec {
    Spec {
        name: "git",
        who: "fetching Pomes",
        program: PathBuf::from("git"),
        parse: git_version,
        min_major: None,
        needs: "git".into(),
        missing_help: None,
        too_old_help: None,
    }
}

fn pome_report(env: &dyn Fn(&str) -> Option<OsString>) -> PomeReport {
    let var = |key: &str| env(key).and_then(|v| v.into_string().ok());
    let config = Config::from_env(var, Some(PathBuf::new()));

    let git_required = config
        .as_ref()
        .is_ok_and(|c| !c.private.is_empty() || c.sources == [Source::Direct]);

    let settings = config.map(|c| PomeSettings {
        proxy: c
            .sources
            .iter()
            .map(|s| match s {
                Source::Proxy(url) => mask_userinfo(url),
                Source::Direct => "direct".to_owned(),
            })
            .collect(),
        sumdb: c.sumdb.map(|s| mask_userinfo(&s.url)),
        private: c.private.len(),
    });

    PomeReport {
        settings,
        git: check(&git_spec(), env),
        git_required,
    }
}

fn mask_userinfo(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };

    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));

    match authority.rsplit_once('@') {
        Some((_, host)) => format!("{scheme}://***@{host}{path}"),
        None => url.to_owned(),
    }
}

fn report(required: Option<Target>, env: &dyn Fn(&str) -> Option<OsString>) -> Report {
    let specs: [(Target, Vec<Spec>); 3] = [
        (Target::Js, vec![node_spec(env)]),
        (Target::C, vec![cc_spec(env)]),
        (Target::Wasm, vec![wasmtime_spec(env)]),
    ];
    let targets = specs
        .into_iter()
        .map(|(target, spec)| TargetReport {
            target,
            required: required == Some(target),
            programs: spec.iter().map(|spec| check(spec, env)).collect(),
        })
        .collect();

    Report {
        targets,
        pome: pome_report(env),
    }
}

fn exit_code(report: &Report) -> u8 {
    let programs_pass = report
        .targets
        .iter()
        .filter(|t| t.required)
        .flat_map(|t| &t.programs)
        .all(|p| p.status == Status::Ok);
    let git_passes = !report.pome.git_required || report.pome.git.status == Status::Ok;
    if programs_pass && git_passes { 0 } else { 1 }
}

fn paint(color: bool, code: &str, text: &str) -> String {
    if color {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

/// One line under a heading: the label padded to 9 columns, then the text. With colors, a result
/// has a mark in place of the first space of the line (`[cmd.doctor]`): `✓` when it passes and `✗`
/// when it fails. Both marks are one column wide in every terminal (East Asian Width N), so the
/// columns stay aligned.
fn item(out: &mut String, color: bool, code: &str, mark: Option<char>, label: &str, text: &str) {
    let indent = match mark {
        Some(mark) if color => paint(color, code, &format!("{mark} ")),
        _ => "  ".to_owned(),
    };
    let label = paint(color, code, &format!("{label:<9}"));
    writeln!(out, "{indent}{label}{text}").unwrap();
}

fn help(out: &mut String, text: &str) {
    writeln!(out, "  {:9}{text}", "").unwrap();
}

/// A heading: the name of a target or `pome`, and ` (required)` in bold when it is required.
fn heading(out: &mut String, color: bool, name: &str, required: bool) {
    if required {
        writeln!(out, "{name} {}", paint(color, "1", "(required)")).unwrap();
    } else {
        writeln!(out, "{name}").unwrap();
    }
}

/// The line of one program, and the line of its help when it fails.
fn program(out: &mut String, color: bool, p: &ProgramCheck) {
    match &p.message {
        None => {
            let version = p
                .version
                .as_deref()
                .map(|v| format!(" {v}"))
                .unwrap_or_default();
            let text = format!("{}{version} ({})", p.name, p.executable.display());
            item(out, color, "32", Some('✓'), p.status.label(), &text);
        }
        Some(message) => {
            item(out, color, "31", Some('✗'), p.status.label(), message);
            if let Some(text) = &p.help {
                help(out, text);
            }
        }
    }
}

/// The report for a person at a terminal (`[cmd.doctor]`).
fn render_text(report: &Report, version: &str, color: bool) -> String {
    let mut out = String::new();
    writeln!(out, "emela {version}\n").unwrap();
    for target in &report.targets {
        heading(&mut out, color, target.target.name(), target.required);
        for p in &target.programs {
            program(&mut out, color, p);
        }
    }

    writeln!(out).unwrap();
    heading(&mut out, color, "pome", report.pome.git_required);
    match &report.pome.settings {
        Ok(settings) => {
            let proxy = if settings.proxy.is_empty() {
                "off".to_owned()
            } else {
                settings.proxy.join(", ")
            };
            let sumdb = settings.sumdb.as_deref().unwrap_or("off");
            let private = match settings.private {
                1 => "1 prefix".to_owned(),
                n => format!("{n} prefixes"),
            };
            // The settings are not a result, so their labels are not colored.
            item(&mut out, false, "", None, "proxy", &proxy);
            item(&mut out, false, "", None, "sumdb", sumdb);
            item(&mut out, false, "", None, "private", &private);
        }
        Err(error) => item(&mut out, color, "31", Some('✗'), "failed", error),
    }
    program(&mut out, color, &report.pome.git);
    out
}

fn program_json(p: &ProgramCheck) -> serde_json::Value {
    serde_json::json!({
        "name": p.name,
        "status": p.status.label(),
        "executable": p.executable.display().to_string(),
        "version": p.version,
        "message": p.message,
        "help": p.help,
    })
}

/// The report as one JSON object, for tools (`[cmd.doctor]`).
///
/// When the Pome settings cannot be read, `proxy`, `sumdb` and `private` are `null`: the
/// specification shows no key for the error yet.
fn render_json(report: &Report, version: &str) -> String {
    let targets: Vec<_> = report
        .targets
        .iter()
        .map(|t| {
            serde_json::json!({
                "target": t.target.name(),
                "required": t.required,
                "programs": t.programs.iter().map(program_json).collect::<Vec<_>>(),
            })
        })
        .collect();
    let settings = report.pome.settings.as_ref().ok();
    let value = serde_json::json!({
        "emela": version,
        "targets": targets,
        "pome": {
            "proxy": settings.map(|s| &s.proxy),
            "sumdb": settings.map(|s| &s.sumdb),
            "private": settings.map(|s| s.private),
            "git": program_json(&report.pome.git),
        },
    });
    serde_json::to_string_pretty(&value).expect("a JSON value is always written")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The parsed version as plain values, so that `Version` needs no derives for the tests.
    fn parsed(version: Option<Version>) -> Option<(String, Option<u32>)> {
        version.map(|v| (v.text, v.major))
    }

    fn some(text: &str, major: Option<u32>) -> Option<(String, Option<u32>)> {
        Some((text.to_owned(), major))
    }

    #[test]
    fn node_versions() {
        assert_eq!(
            parsed(node_version("v24.11.0\n")),
            some("24.11.0", Some(24))
        );
        assert_eq!(
            parsed(node_version("v18.0.0\r\n")),
            some("18.0.0", Some(18))
        );
        // Without the leading `v`, as a wrapper set in `EMELA_NODE` may print it.
        assert_eq!(parsed(node_version("24.11.0\n")), some("24.11.0", Some(24)));
        assert_eq!(parsed(node_version("node\n")), None);
        assert_eq!(parsed(node_version("")), None);
    }

    #[test]
    fn wasmtime_versions() {
        assert_eq!(
            parsed(wasmtime_version("wasmtime 49.0.3 (a1b2c3d 2026-01-20)\n")),
            some("49.0.3", Some(49))
        );
        assert_eq!(
            parsed(wasmtime_version("wasmtime-cli 20.0.0\n")),
            some("20.0.0", Some(20))
        );
        assert_eq!(parsed(wasmtime_version("wasmtime\n")), None);
        assert_eq!(parsed(wasmtime_version("")), None);
    }

    #[test]
    fn git_versions() {
        assert_eq!(
            parsed(git_version("git version 2.51.0\n")),
            some("2.51.0", Some(2))
        );
        assert_eq!(
            parsed(git_version("git version 2.39.5 (Apple Git-154)\n")),
            some("2.39.5", Some(2))
        );
        assert_eq!(parsed(git_version("git\n")), None);
        assert_eq!(parsed(git_version("")), None);
    }

    #[test]
    fn cc_versions() {
        // The first line that is not blank, as it is; no major version.
        assert_eq!(
            parsed(cc_version(
                "Apple clang version 17.0.0 (clang-1700.0.13.5)\nTarget: arm64-apple-darwin25.6.0\n"
            )),
            some("Apple clang version 17.0.0 (clang-1700.0.13.5)", None)
        );
        assert_eq!(
            parsed(cc_version("gcc (GCC) 15.2.0\r\nCopyright (C) 2025\n")),
            some("gcc (GCC) 15.2.0", None)
        );
        assert_eq!(parsed(cc_version("\n\ncc 1.0\n")), some("cc 1.0", None));
        assert_eq!(parsed(cc_version("  \n")), None);
        assert_eq!(parsed(cc_version("")), None);
    }
}

/// Tests that start programs: fake executables written to a temporary directory.
#[cfg(all(test, unix))]
mod process_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Writes `script` to `dir/name` with the permission bits `mode`. The file is closed before
    /// it is started, so it is not busy.
    fn fake(dir: &Path, name: &str, script: &str, mode: u32) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    /// An environment whose only variable is `PATH`, made of `dirs` in order.
    fn path_env(dirs: &[&Path]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let path = std::env::join_paths(dirs).unwrap();
        move |key| (key == "PATH").then(|| path.clone())
    }

    fn no_env(_: &str) -> Option<OsString> {
        None
    }

    // find_executable

    #[test]
    fn finds_a_program_on_path() {
        let dir = tempfile::tempdir().unwrap();
        let node = fake(dir.path(), "node", "#!/bin/sh\n", 0o755);
        let found = find_executable(Path::new("node"), &path_env(&[dir.path()]));
        assert_eq!(found, Some(node));
    }

    #[test]
    fn the_first_directory_on_path_wins() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let node = fake(first.path(), "node", "#!/bin/sh\n", 0o755);
        fake(second.path(), "node", "#!/bin/sh\n", 0o755);
        let found = find_executable(Path::new("node"), &path_env(&[first.path(), second.path()]));
        assert_eq!(found, Some(node));
    }

    #[test]
    fn skips_files_that_are_not_executable_and_directories() {
        let plain = tempfile::tempdir().unwrap();
        let nested = tempfile::tempdir().unwrap();
        let real = tempfile::tempdir().unwrap();
        fake(plain.path(), "node", "#!/bin/sh\n", 0o644);
        std::fs::create_dir(nested.path().join("node")).unwrap();
        let node = fake(real.path(), "node", "#!/bin/sh\n", 0o755);
        let env = path_env(&[plain.path(), nested.path(), real.path()]);
        assert_eq!(find_executable(Path::new("node"), &env), Some(node));
    }

    #[test]
    fn a_program_not_on_path_is_missing() {
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            find_executable(Path::new("node"), &path_env(&[empty.path()])),
            None
        );
        assert_eq!(find_executable(Path::new("node"), &no_env), None);
    }

    #[test]
    fn a_path_with_a_directory_is_not_looked_up_on_path() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let node = fake(dir.path(), "node", "#!/bin/sh\n", 0o755);
        fake(other.path(), "node", "#!/bin/sh\n", 0o755);
        let env = path_env(&[other.path()]);
        assert_eq!(find_executable(&node, &env), Some(node.clone()));
        assert_eq!(find_executable(&dir.path().join("missing"), &env), None);
    }

    #[test]
    fn a_relative_path_with_a_directory_is_not_looked_up_on_path() {
        // `bin/node` exists only under a directory on PATH, not under the working directory.
        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir(other.path().join("bin")).unwrap();
        fake(&other.path().join("bin"), "node", "#!/bin/sh\n", 0o755);
        let program = Path::new("bin/node");
        assert!(!program.exists(), "the working directory has a `bin/node`");
        assert_eq!(find_executable(program, &path_env(&[other.path()])), None);
    }

    // probe

    #[test]
    fn probe_reads_the_version_from_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let node = fake(dir.path(), "node", "#!/bin/sh\necho v20.1.0\n", 0o755);
        let version = probe(&node, node_version).unwrap().unwrap();
        assert_eq!((version.text.as_str(), version.major), ("20.1.0", Some(20)));
    }

    #[test]
    fn probe_falls_back_to_stderr() {
        // Some programs write their version to standard error.
        let dir = tempfile::tempdir().unwrap();
        let git = fake(
            dir.path(),
            "git",
            "#!/bin/sh\necho 'git version 2.51.0' >&2\n",
            0o755,
        );
        let version = probe(&git, git_version).unwrap().unwrap();
        assert_eq!((version.text.as_str(), version.major), ("2.51.0", Some(2)));
    }

    #[test]
    fn probe_of_a_program_that_starts_without_a_version() {
        let dir = tempfile::tempdir().unwrap();
        let chatty = fake(dir.path(), "chatty", "#!/bin/sh\necho hello\n", 0o755);
        let failing = fake(dir.path(), "failing", "#!/bin/sh\nexit 3\n", 0o755);
        assert!(probe(&chatty, node_version).unwrap().is_none());
        assert!(probe(&failing, node_version).unwrap().is_none());
    }

    #[test]
    fn probe_of_a_program_that_cannot_start() {
        let dir = tempfile::tempdir().unwrap();
        let plain = fake(dir.path(), "node", "#!/bin/sh\necho v24.0.0\n", 0o644);
        let broken = fake(dir.path(), "broken", "#!/nonexistent/sh\n", 0o755);
        assert!(probe(&plain, node_version).is_err());
        assert!(probe(&broken, node_version).is_err());
    }

    // status

    fn probed(word: &str) -> std::io::Result<Option<Version>> {
        Ok(number(word))
    }

    #[test]
    fn statuses() {
        let cannot_start = Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert_eq!(status(None, Some(22)), Status::Missing);
        assert_eq!(status(Some(&cannot_start), Some(22)), Status::Failed);
        assert_eq!(status(Some(&probed("20.1.0")), Some(22)), Status::TooOld);
        assert_eq!(status(Some(&probed("22.0.0")), Some(22)), Status::Ok);
        assert_eq!(status(Some(&probed("24.11.0")), Some(22)), Status::Ok);
    }

    #[test]
    fn a_version_that_cannot_be_read_passes() {
        assert_eq!(status(Some(&Ok(None)), Some(22)), Status::Ok);
        let cc = Ok(cc_version("Apple clang version 17.0.0\n"));
        assert_eq!(status(Some(&cc), Some(22)), Status::Ok);
    }

    #[test]
    fn a_program_without_a_minimum_passes_when_it_starts() {
        assert_eq!(status(Some(&probed("1.0")), None), Status::Ok);
        assert_eq!(status(Some(&Ok(None)), None), Status::Ok);
        assert_eq!(status(None, None), Status::Missing);
    }

    // Resolving each program from the environment, by the rule of the command that uses it

    /// An environment with exactly the variables `pairs`.
    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let pairs: Vec<(String, OsString)> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), OsString::from(value)))
            .collect();
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, value)| value.clone())
        }
    }

    #[test]
    fn programs_are_resolved_from_the_environment() {
        let env = vars(&[
            ("EMELA_NODE", "/opt/node/bin/node"),
            ("EMELA_WASMTIME", "/opt/wasmtime/wasmtime"),
            ("EMELA_CC", "clang -fsanitize=address"),
        ]);
        assert_eq!(
            crate::run::node_program_in(&env),
            PathBuf::from("/opt/node/bin/node")
        );
        assert_eq!(
            crate::run::wasmtime_program_in(&env),
            PathBuf::from("/opt/wasmtime/wasmtime")
        );
        assert_eq!(
            crate::c::cc_command_in(&env),
            (
                PathBuf::from("clang"),
                vec!["-fsanitize=address".to_owned()]
            )
        );
    }

    #[test]
    fn programs_have_defaults_when_the_variables_are_unset_or_blank() {
        for env in [
            vars(&[]),
            vars(&[
                ("EMELA_NODE", ""),
                ("EMELA_WASMTIME", ""),
                ("EMELA_CC", "  "),
            ]),
        ] {
            assert_eq!(crate::run::node_program_in(&env), PathBuf::from("node"));
            assert_eq!(
                crate::run::wasmtime_program_in(&env),
                PathBuf::from("wasmtime")
            );
            assert_eq!(
                crate::c::cc_command_in(&env),
                (PathBuf::from("cc"), Vec::new())
            );
        }
    }

    // check: one program, from finding it to the message and help

    /// `vars` with `PATH` set to `dir`.
    fn on_path(dir: &Path, pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let mut all = vec![("PATH", dir.to_str().unwrap())];
        all.extend_from_slice(pairs);
        vars(&all)
    }

    #[test]
    fn a_node_that_is_new_enough() {
        let dir = tempfile::tempdir().unwrap();
        let node = fake(dir.path(), "node", "#!/bin/sh\necho v24.11.0\n", 0o755);
        let env = on_path(dir.path(), &[]);
        let check = check(&node_spec(&env), &env);
        assert_eq!(check.name, "node");
        assert_eq!(check.status, Status::Ok);
        assert_eq!(check.executable, node);
        assert_eq!(check.version.as_deref(), Some("24.11.0"));
        assert_eq!(check.message, None);
        assert_eq!(check.help, None);
    }

    #[test]
    fn a_node_that_is_too_old() {
        let dir = tempfile::tempdir().unwrap();
        let node = fake(dir.path(), "node", "#!/bin/sh\necho v20.1.0\n", 0o755);
        let env = on_path(dir.path(), &[]);
        let check = check(&node_spec(&env), &env);
        assert_eq!(check.status, Status::TooOld);
        assert_eq!(check.version.as_deref(), Some("20.1.0"));
        assert_eq!(
            check.message,
            Some(format!(
                "target `js` needs node 22 or later, but `{}` is 20.1.0",
                node.display()
            ))
        );
        // The note of E0904 for a node that is too old.
        assert_eq!(
            check.help.as_deref(),
            Some("install Node.js 22 or later, or set EMELA_NODE to a newer node executable")
        );
    }

    #[test]
    fn a_missing_wasmtime() {
        // The example of `[cmd.doctor]`.
        let empty = tempfile::tempdir().unwrap();
        let env = on_path(empty.path(), &[]);
        let check = check(&wasmtime_spec(&env), &env);
        assert_eq!(check.name, "wasmtime");
        assert_eq!(check.status, Status::Missing);
        assert_eq!(check.executable, PathBuf::from("wasmtime"));
        assert_eq!(check.version, None);
        assert_eq!(
            check.message.as_deref(),
            Some("target `wasm` needs wasmtime 49 or later, but `wasmtime` is not on PATH")
        );
        assert_eq!(
            check.help.as_deref(),
            Some(
                "install Wasmtime 49 or later and add it to PATH, or set EMELA_WASMTIME to the wasmtime executable"
            )
        );
    }

    #[test]
    fn a_missing_program_named_by_a_path() {
        let empty = tempfile::tempdir().unwrap();
        let env = on_path(empty.path(), &[("EMELA_WASMTIME", "/nonexistent/wasmtime")]);
        let check = check(&wasmtime_spec(&env), &env);
        assert_eq!(check.status, Status::Missing);
        assert_eq!(check.executable, PathBuf::from("/nonexistent/wasmtime"));
        assert_eq!(
            check.message.as_deref(),
            Some(
                "target `wasm` needs wasmtime 49 or later, but `/nonexistent/wasmtime` is not an executable file"
            )
        );
    }

    #[test]
    fn a_wasmtime_whose_version_cannot_be_read_passes() {
        let dir = tempfile::tempdir().unwrap();
        fake(dir.path(), "wasmtime", "#!/bin/sh\necho hello\n", 0o755);
        let env = on_path(dir.path(), &[]);
        let check = check(&wasmtime_spec(&env), &env);
        assert_eq!(check.status, Status::Ok);
        assert_eq!(check.version, None);
        assert_eq!(check.message, None);
        assert_eq!(check.help, None);
    }

    #[test]
    fn a_node_that_cannot_start() {
        let dir = tempfile::tempdir().unwrap();
        let node = fake(dir.path(), "node", "#!/nonexistent/sh\n", 0o755);
        let env = on_path(dir.path(), &[]);
        let check = check(&node_spec(&env), &env);
        assert_eq!(check.status, Status::Failed);
        assert_eq!(check.executable, node);
        let prefix = format!(
            "target `js` needs node 22 or later, but `{}` cannot be started: ",
            node.display()
        );
        let message = check.message.unwrap();
        assert!(message.starts_with(&prefix), "{message}");
        // The diagnostic for a node that cannot start has no help.
        assert_eq!(check.help, None);
    }

    #[test]
    fn a_c_compiler_from_emela_cc() {
        // Only the program of `EMELA_CC` is started; its flags are not passed to `--version`.
        let dir = tempfile::tempdir().unwrap();
        let cc = fake(
            dir.path(),
            "clang",
            "#!/bin/sh\n[ \"$1\" = --version ] || exit 1\necho 'Apple clang version 17.0.0 (clang-1700)'\necho 'Target: arm64'\n",
            0o755,
        );
        let empty = tempfile::tempdir().unwrap();
        let program = format!("{} -fsanitize=address", cc.display());
        let env = on_path(empty.path(), &[("EMELA_CC", &program)]);
        let check = check(&cc_spec(&env), &env);
        assert_eq!(check.name, "cc");
        assert_eq!(check.status, Status::Ok);
        assert_eq!(check.executable, cc);
        assert_eq!(
            check.version.as_deref(),
            Some("Apple clang version 17.0.0 (clang-1700)")
        );
    }

    #[test]
    fn a_missing_c_compiler() {
        let empty = tempfile::tempdir().unwrap();
        let env = on_path(empty.path(), &[]);
        let check = check(&cc_spec(&env), &env);
        assert_eq!(check.status, Status::Missing);
        assert_eq!(check.executable, PathBuf::from("cc"));
        assert_eq!(
            check.message.as_deref(),
            Some("target `c` needs a C compiler, but `cc` is not on PATH")
        );
        // The note of E0907.
        assert_eq!(
            check.help.as_deref(),
            Some(
                "install a C11 compiler and add it to PATH as `cc`, or set EMELA_CC to the compiler"
            )
        );
    }

    // The Pome section

    #[test]
    fn userinfo_is_masked() {
        assert_eq!(
            mask_userinfo("https://user:token@p.example/x"),
            "https://***@p.example/x"
        );
        assert_eq!(
            mask_userinfo("https://user@p.example"),
            "https://***@p.example"
        );
        assert_eq!(
            mask_userinfo("http://user:token@127.0.0.1:8080/"),
            "http://***@127.0.0.1:8080/"
        );
        // An `@` after the host is not userinfo.
        assert_eq!(
            mask_userinfo("https://p.example/a@b"),
            "https://p.example/a@b"
        );
        assert_eq!(mask_userinfo("https://p.example"), "https://p.example");
        assert_eq!(mask_userinfo("direct"), "direct");
    }

    #[test]
    fn the_default_pome_settings() {
        let empty = tempfile::tempdir().unwrap();
        let pome = pome_report(&on_path(empty.path(), &[]));
        let settings = pome.settings.unwrap();
        assert_eq!(settings.proxy, ["https://pomes.emela.dev", "direct"]);
        assert_eq!(settings.sumdb.as_deref(), Some("https://sum.emela.dev"));
        assert_eq!(settings.private, 0);
        assert!(!pome.git_required);
    }

    #[test]
    fn pome_settings_that_are_off() {
        let empty = tempfile::tempdir().unwrap();
        let env = on_path(
            empty.path(),
            &[("EMELA_POMEPROXY", "off"), ("EMELA_POMESUMDB", "off")],
        );
        let pome = pome_report(&env);
        let settings = pome.settings.unwrap();
        assert!(settings.proxy.is_empty());
        assert_eq!(settings.sumdb, None);
        // Nothing to fetch from, so nothing needs git.
        assert!(!pome.git_required);
    }

    #[test]
    fn pome_urls_are_masked() {
        let key = emela_pome::fetch::config::DEFAULT_SUMDB
            .split_whitespace()
            .next()
            .unwrap();
        let sumdb = format!("{key} https://user:token@sum.example/");
        let empty = tempfile::tempdir().unwrap();
        let env = on_path(
            empty.path(),
            &[
                ("EMELA_POMEPROXY", "https://user:token@p.example/,direct"),
                ("EMELA_POMESUMDB", &sumdb),
            ],
        );
        let settings = pome_report(&env).settings.unwrap();
        assert_eq!(settings.proxy, ["https://***@p.example", "direct"]);
        assert_eq!(settings.sumdb.as_deref(), Some("https://***@sum.example"));
    }

    #[test]
    fn private_prefixes_are_counted_and_need_git() {
        let empty = tempfile::tempdir().unwrap();
        let env = on_path(
            empty.path(),
            &[("EMELA_POMEPRIVATE", "github.com/acme, git.corp.example")],
        );
        let pome = pome_report(&env);
        assert_eq!(pome.settings.unwrap().private, 2);
        assert!(pome.git_required);
    }

    #[test]
    fn direct_alone_needs_git() {
        let empty = tempfile::tempdir().unwrap();
        let env = on_path(empty.path(), &[("EMELA_POMEPROXY", "direct")]);
        assert!(pome_report(&env).git_required);
    }

    #[test]
    fn invalid_pome_settings_are_reported() {
        let empty = tempfile::tempdir().unwrap();
        let env = on_path(empty.path(), &[("EMELA_POMEPROXY", "ftp://p.example")]);
        let pome = pome_report(&env);
        let error = pome.settings.unwrap_err();
        assert!(error.contains("EMELA_POMEPROXY"), "{error}");
        assert!(!pome.git_required);
    }

    #[test]
    fn pome_settings_do_not_need_a_home_directory() {
        // The cache directory is not reported, so a missing home is not an error.
        let empty = tempfile::tempdir().unwrap();
        let pome = pome_report(&on_path(empty.path(), &[]));
        assert!(pome.settings.is_ok());
    }

    #[test]
    fn git_is_checked() {
        let dir = tempfile::tempdir().unwrap();
        let git = fake(
            dir.path(),
            "git",
            "#!/bin/sh\necho 'git version 2.51.0'\n",
            0o755,
        );
        let pome = pome_report(&on_path(dir.path(), &[]));
        assert_eq!(pome.git.name, "git");
        assert_eq!(pome.git.status, Status::Ok);
        assert_eq!(pome.git.executable, git);
        assert_eq!(pome.git.version.as_deref(), Some("2.51.0"));
    }

    #[test]
    fn a_missing_git() {
        let empty = tempfile::tempdir().unwrap();
        let pome = pome_report(&on_path(empty.path(), &[]));
        assert_eq!(pome.git.status, Status::Missing);
        assert_eq!(
            pome.git.message.as_deref(),
            Some("fetching Pomes needs git, but `git` is not on PATH")
        );
        // No command reports a diagnostic with help for git.
        assert_eq!(pome.git.help, None);
    }

    // The whole report

    fn targets(report: &Report) -> Vec<(Target, bool)> {
        report
            .targets
            .iter()
            .map(|t| (t.target, t.required))
            .collect()
    }

    #[test]
    fn no_target_is_required_by_default() {
        let empty = tempfile::tempdir().unwrap();
        let report = report(None, &on_path(empty.path(), &[]));
        assert_eq!(
            targets(&report),
            [
                (Target::Js, false),
                (Target::C, false),
                (Target::Wasm, false)
            ]
        );
    }

    #[test]
    fn the_required_target_is_marked() {
        let empty = tempfile::tempdir().unwrap();
        let report = report(Some(Target::Wasm), &on_path(empty.path(), &[]));
        assert_eq!(
            targets(&report),
            [
                (Target::Js, false),
                (Target::C, false),
                (Target::Wasm, true)
            ]
        );
        let names: Vec<_> = report.targets[2].programs.iter().map(|p| p.name).collect();
        assert_eq!(names, ["wasmtime"]);
    }

    #[test]
    fn a_missing_program_of_a_required_target_fails() {
        let empty = tempfile::tempdir().unwrap();
        let env = on_path(empty.path(), &[]);
        assert_eq!(exit_code(&report(Some(Target::Wasm), &env)), 1);
    }

    #[test]
    fn missing_programs_of_other_targets_pass() {
        let dir = tempfile::tempdir().unwrap();
        fake(dir.path(), "node", "#!/bin/sh\necho v24.11.0\n", 0o755);
        let env = on_path(dir.path(), &[]);
        // cc, wasmtime and git are missing, but only js is required.
        assert_eq!(exit_code(&report(Some(Target::Js), &env)), 0);
        assert_eq!(exit_code(&report(None, &env)), 0);
    }

    #[test]
    fn a_too_old_program_of_a_required_target_fails() {
        let dir = tempfile::tempdir().unwrap();
        fake(dir.path(), "node", "#!/bin/sh\necho v20.1.0\n", 0o755);
        let env = on_path(dir.path(), &[]);
        assert_eq!(exit_code(&report(Some(Target::Js), &env)), 1);
    }

    #[test]
    fn a_missing_git_fails_only_when_it_is_required() {
        let empty = tempfile::tempdir().unwrap();
        let not_required = on_path(empty.path(), &[]);
        assert_eq!(exit_code(&report(None, &not_required)), 0);
        let required = on_path(empty.path(), &[("EMELA_POMEPRIVATE", "github.com/acme")]);
        assert_eq!(exit_code(&report(None, &required)), 1);

        let dir = tempfile::tempdir().unwrap();
        fake(
            dir.path(),
            "git",
            "#!/bin/sh\necho 'git version 2.51.0'\n",
            0o755,
        );
        let present = on_path(dir.path(), &[("EMELA_POMEPRIVATE", "github.com/acme")]);
        assert_eq!(exit_code(&report(None, &present)), 0);
    }

    #[test]
    fn invalid_pome_settings_do_not_change_the_exit_code() {
        // `[cmd.doctor]` sets the exit code from the required programs and git alone.
        let empty = tempfile::tempdir().unwrap();
        let env = on_path(empty.path(), &[("EMELA_POMEPROXY", "ftp://p.example")]);
        assert_eq!(exit_code(&report(None, &env)), 0);
    }

    // Output

    const EMELA: &str = "0.28.0+4e84642";

    /// Every program is missing, `wasm` is required, and the Pome settings are the defaults.
    fn nothing_installed() -> Report {
        let empty = tempfile::tempdir().unwrap();
        report(Some(Target::Wasm), &on_path(empty.path(), &[]))
    }

    /// node and cc pass, wasmtime is too old, git passes and is required by a private prefix,
    /// and the proxy and the checksum database are off. Paths under the directory are shown as
    /// `[DIR]`.
    fn some_installed(render: impl Fn(&Report) -> String) -> String {
        let dir = tempfile::tempdir().unwrap();
        fake(dir.path(), "node", "#!/bin/sh\necho v24.11.0\n", 0o755);
        fake(
            dir.path(),
            "cc",
            "#!/bin/sh\necho 'Apple clang version 17.0.0 (clang-1700)'\necho 'Target: arm64'\n",
            0o755,
        );
        fake(
            dir.path(),
            "wasmtime",
            "#!/bin/sh\necho 'wasmtime 20.0.0'\n",
            0o755,
        );
        fake(
            dir.path(),
            "git",
            "#!/bin/sh\necho 'git version 2.51.0'\n",
            0o755,
        );
        let env = on_path(
            dir.path(),
            &[
                ("EMELA_POMEPROXY", "off"),
                ("EMELA_POMESUMDB", "off"),
                ("EMELA_POMEPRIVATE", "github.com/acme"),
            ],
        );
        render(&report(Some(Target::Js), &env)).replace(dir.path().to_str().unwrap(), "[DIR]")
    }

    #[test]
    fn text_when_nothing_is_installed() {
        insta::assert_snapshot!(render_text(&nothing_installed(), EMELA, false), @r"
        emela 0.28.0+4e84642

        js
          missing  target `js` needs node 22 or later, but `node` is not on PATH
                   install Node.js 22 or later and add it to PATH, or set EMELA_NODE to the node executable
        c
          missing  target `c` needs a C compiler, but `cc` is not on PATH
                   install a C11 compiler and add it to PATH as `cc`, or set EMELA_CC to the compiler
        wasm (required)
          missing  target `wasm` needs wasmtime 49 or later, but `wasmtime` is not on PATH
                   install Wasmtime 49 or later and add it to PATH, or set EMELA_WASMTIME to the wasmtime executable

        pome
          proxy    https://pomes.emela.dev, direct
          sumdb    https://sum.emela.dev
          private  0 prefixes
          missing  fetching Pomes needs git, but `git` is not on PATH
        ");
    }

    #[test]
    fn text_when_some_are_installed() {
        let text = some_installed(|report| render_text(report, EMELA, false));
        insta::assert_snapshot!(text, @r"
        emela 0.28.0+4e84642

        js (required)
          ok       node 24.11.0 ([DIR]/node)
        c
          ok       cc Apple clang version 17.0.0 (clang-1700) ([DIR]/cc)
        wasm
          too-old  target `wasm` needs wasmtime 49 or later, but `[DIR]/wasmtime` is 20.0.0
                   install Wasmtime 49 or later, or set EMELA_WASMTIME to a newer wasmtime executable

        pome (required)
          proxy    off
          sumdb    off
          private  1 prefix
          ok       git 2.51.0 ([DIR]/git)
        ");
    }

    #[test]
    fn text_of_a_program_without_a_version() {
        let dir = tempfile::tempdir().unwrap();
        fake(dir.path(), "wasmtime", "#!/bin/sh\necho hello\n", 0o755);
        let report = report(None, &on_path(dir.path(), &[]));
        let text =
            render_text(&report, EMELA, false).replace(dir.path().to_str().unwrap(), "[DIR]");
        assert!(
            text.contains("\nwasm\n  ok       wasmtime ([DIR]/wasmtime)\n"),
            "{text}"
        );
    }

    #[test]
    fn text_of_invalid_pome_settings() {
        let empty = tempfile::tempdir().unwrap();
        let env = on_path(empty.path(), &[("EMELA_POMEPROXY", "ftp://p.example")]);
        let text = render_text(&report(None, &env), EMELA, false);
        assert!(
            text.ends_with(
                "\npome\n  failed   EMELA_POMEPROXY: `ftp://p.example` is neither a URL nor `direct`\n  missing  fetching Pomes needs git, but `git` is not on PATH\n"
            ),
            "{text}"
        );
    }

    /// `text` without its ANSI escape sequences (`ESC [ ... m`).
    fn strip_colors(text: &str) -> String {
        let mut plain = String::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                chars.by_ref().find(|&c| c == 'm');
            } else {
                plain.push(c);
            }
        }
        plain
    }

    /// `text` without the marks that replace the first space of a line (`✓ ok`, `✗ missing`).
    fn strip_marks(text: &str) -> String {
        text.lines()
            .map(|line| {
                let plain = line
                    .strip_prefix("✓ ")
                    .or_else(|| line.strip_prefix("✗ "))
                    .map(|rest| format!("  {rest}"));
                plain.unwrap_or_else(|| line.to_owned()) + "\n"
            })
            .collect()
    }

    #[test]
    fn colors_and_marks_only_decorate_the_text() {
        let report = nothing_installed();
        let colored = render_text(&report, EMELA, true);
        assert!(colored.contains('\x1b'), "{colored}");
        assert_eq!(
            strip_marks(&strip_colors(&colored)),
            render_text(&report, EMELA, false)
        );
    }

    #[test]
    fn marks_go_before_the_results() {
        let text = strip_colors(&some_installed(|report| render_text(report, EMELA, true)));
        insta::assert_snapshot!(text, @r"
        emela 0.28.0+4e84642

        js (required)
        ✓ ok       node 24.11.0 ([DIR]/node)
        c
        ✓ ok       cc Apple clang version 17.0.0 (clang-1700) ([DIR]/cc)
        wasm
        ✗ too-old  target `wasm` needs wasmtime 49 or later, but `[DIR]/wasmtime` is 20.0.0
                   install Wasmtime 49 or later, or set EMELA_WASMTIME to a newer wasmtime executable

        pome (required)
          proxy    off
          sumdb    off
          private  1 prefix
        ✓ ok       git 2.51.0 ([DIR]/git)
        ");
    }

    #[test]
    fn no_marks_without_colors() {
        let text = some_installed(|report| render_text(report, EMELA, false));
        assert!(!text.contains('✓') && !text.contains('✗'), "{text}");
    }

    #[test]
    fn json_when_nothing_is_installed() {
        let json: serde_json::Value =
            serde_json::from_str(&render_json(&nothing_installed(), EMELA)).unwrap();
        let missing = |name: &str, message: &str, help: &str| {
            serde_json::json!({
                "name": name,
                "status": "missing",
                "executable": name,
                "version": null,
                "message": message,
                "help": help,
            })
        };
        assert_eq!(
            json,
            serde_json::json!({
                "emela": "0.28.0+4e84642",
                "targets": [
                    {
                        "target": "js",
                        "required": false,
                        "programs": [missing(
                            "node",
                            "target `js` needs node 22 or later, but `node` is not on PATH",
                            "install Node.js 22 or later and add it to PATH, or set EMELA_NODE to the node executable",
                        )],
                    },
                    {
                        "target": "c",
                        "required": false,
                        "programs": [missing(
                            "cc",
                            "target `c` needs a C compiler, but `cc` is not on PATH",
                            "install a C11 compiler and add it to PATH as `cc`, or set EMELA_CC to the compiler",
                        )],
                    },
                    {
                        "target": "wasm",
                        "required": true,
                        "programs": [missing(
                            "wasmtime",
                            "target `wasm` needs wasmtime 49 or later, but `wasmtime` is not on PATH",
                            "install Wasmtime 49 or later and add it to PATH, or set EMELA_WASMTIME to the wasmtime executable",
                        )],
                    },
                ],
                "pome": {
                    "proxy": ["https://pomes.emela.dev", "direct"],
                    "sumdb": "https://sum.emela.dev",
                    "private": 0,
                    "git": {
                        "name": "git",
                        "status": "missing",
                        "executable": "git",
                        "version": null,
                        "message": "fetching Pomes needs git, but `git` is not on PATH",
                        "help": null,
                    },
                },
            })
        );
    }

    #[test]
    fn json_when_some_are_installed() {
        let json: serde_json::Value =
            serde_json::from_str(&some_installed(|report| render_json(report, EMELA))).unwrap();
        assert_eq!(
            json["targets"][0]["programs"][0],
            serde_json::json!({
                "name": "node",
                "status": "ok",
                "executable": "[DIR]/node",
                "version": "24.11.0",
                "message": null,
                "help": null,
            })
        );
        assert_eq!(json["targets"][2]["programs"][0]["status"], "too-old");
        assert_eq!(json["pome"]["proxy"], serde_json::json!([]));
        assert_eq!(json["pome"]["sumdb"], serde_json::Value::Null);
        assert_eq!(json["pome"]["private"], 1);
        assert_eq!(json["pome"]["git"]["version"], "2.51.0");
    }

    #[test]
    fn private_prefixes_are_not_written() {
        let text = some_installed(|report| render_text(report, EMELA, false));
        let json = some_installed(|report| render_json(report, EMELA));
        assert!(!text.contains("acme"), "{text}");
        assert!(!json.contains("acme"), "{json}");
    }
}
