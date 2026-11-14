use std::ffi::OsString;
use std::fmt::Write;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::Target;
use crate::c::CC_ARGS_ENV;
use emela_pome::fetch::config::{Config, Source};

pub(crate) struct Version {
    pub(crate) text: String,
    pub(crate) major: Option<u32>,
    pub(crate) minor: Option<u32>,
    pub(crate) patch: Option<u32>,
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
    /// The name of the program: as the JS host wrote it (`node`, `bun`), otherwise the name of
    /// the spec, or for the JS host the file name of its command.
    name: String,
    status: Status,
    executable: PathBuf,
    version: Option<String>,
    message: Option<String>,
    help: Option<String>,
    /// The value of `EMELA_CC_ARGS` for the C compiler when it is set, shown on a line of its own
    /// (`[cmd.doctor]`).
    args: Option<String>,
}

struct Spec {
    name: String,
    who: &'static str,
    program: PathBuf,
    probe: Probe,
    /// What the target needs, in the message when the program is missing or cannot start.
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

/// `<major>[.<minor>[.<patch>[...]]]`; the minor and the patch are `None` when they are absent
/// or not a number.
pub(crate) fn number(word: &str) -> Option<Version> {
    let mut parts = word.split('.');
    let major = parts.next().and_then(|s| s.parse::<u32>().ok())?;
    let minor = parts.next().and_then(|s| s.parse::<u32>().ok());
    let patch = parts.next().and_then(|s| s.parse::<u32>().ok());

    Some(Version {
        text: word.to_string(),
        major: Some(major),
        minor,
        patch,
    })
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
        minor: None,
        patch: None,
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

/// How the version of a program is read.
enum Probe {
    /// From `<program> --version`, compared with the minimum major version if there is one.
    VersionFlag {
        parse: fn(&str) -> Option<Version>,
        min_major: Option<u32>,
    },
    /// From inside the JS host, compared with the minimum of the host it reports (`[cmd.build]`).
    JsHost,
}

/// The version that a program reported.
struct Found {
    /// The name the program wrote with its version (`node`, `bun`).
    name: Option<&'static str>,
    version: Version,
    /// When the version is older than the minimum: what the target needs (`Node 22 or later`)
    /// and the version as it reads after "is" in the message (`Node 20.1.0`).
    too_old: Option<(String, String)>,
}

fn find_version(spec: &Spec, exe: &Path) -> io::Result<Option<Found>> {
    match spec.probe {
        Probe::VersionFlag { parse, min_major } => {
            Ok(probe(exe, parse)?.map(|version| compare_major(version, min_major, &spec.needs)))
        }
        Probe::JsHost => Ok(crate::js_host::probe(exe)?.map(|probed| {
            let too_old = probed.too_old().then(|| {
                (
                    format!("{} or later", probed.kind.minimum_text()),
                    probed.describe(),
                )
            });
            Found {
                name: Some(probed.kind.word()),
                version: probed.version,
                too_old,
            }
        })),
    }
}

/// A version read from `--version`, compared with the minimum major version if there is one.
fn compare_major(version: Version, min_major: Option<u32>, needs: &str) -> Found {
    let too_old = min_major
        .filter(|min| version.major.is_some_and(|major| major < *min))
        .map(|_| (needs.to_owned(), version.text.clone()));
    Found {
        name: None,
        version,
        too_old,
    }
}

fn status(probed: Option<&io::Result<Option<Found>>>) -> Status {
    match probed {
        None => Status::Missing,
        Some(Err(_)) => Status::Failed,
        Some(Ok(Some(found))) if found.too_old.is_some() => Status::TooOld,
        Some(Ok(_)) => Status::Ok,
    }
}

fn check(spec: &Spec, env: &dyn Fn(&str) -> Option<OsString>) -> ProgramCheck {
    let executable = find_executable(&spec.program, env);
    let probed = executable.as_ref().map(|exe| find_version(spec, exe));
    let status = status(probed.as_ref());
    let shown = executable.as_ref().unwrap_or(&spec.program);
    let found = probed
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .and_then(|v| v.as_ref());
    let version = found.map(|f| f.version.text.clone());
    let name = found
        .and_then(|f| f.name)
        .map_or_else(|| spec.name.clone(), str::to_owned);
    let error = match &probed {
        Some(Err(e)) => Some(e.to_string()),
        _ => None,
    };
    let lead = |needs: &str| format!("{} needs {needs}, but `{}`", spec.who, shown.display());

    let (message, help) = match status {
        Status::Ok => (None, None),
        Status::Missing => {
            // A name with a directory is not looked up on PATH (see `find_executable`).
            let why = if spec.program.components().count() > 1 {
                "is not an executable file"
            } else {
                "is not on PATH"
            };
            (
                Some(format!("{} {why}", lead(&spec.needs))),
                spec.missing_help.clone(),
            )
        }
        Status::TooOld => {
            let (needs, described) = found.and_then(|f| f.too_old.clone()).unwrap_or_default();
            (
                Some(format!("{} is {described}", lead(&needs))),
                spec.too_old_help.clone(),
            )
        }
        Status::Failed => (
            Some(format!(
                "{} cannot be started: {}",
                lead(&spec.needs),
                error.unwrap_or_default()
            )),
            None,
        ),
    };

    ProgramCheck {
        name,
        status,
        executable: shown.clone(),
        version,
        message,
        help,
        args: None,
    }
}

/// The JS host (`[cmd.build]`). Before it has reported itself, it is named by the file name of
/// its command (`node`, `bun`).
fn js_host_spec(env: &dyn Fn(&str) -> Option<OsString>) -> Spec {
    let program = crate::js_host::js_host_in(env).program;
    let name = program
        .file_name()
        .map_or_else(|| "node".to_owned(), |n| n.to_string_lossy().into_owned());
    Spec {
        name,
        who: "target `js`",
        program,
        probe: Probe::JsHost,
        needs: crate::js_host::NEEDS.to_owned(),
        missing_help: Some(crate::js_host::missing_help()),
        too_old_help: Some(crate::js_host::too_old_help()),
    }
}

fn wasmtime_spec(env: &dyn Fn(&str) -> Option<OsString>) -> Spec {
    Spec {
        name: "wasmtime".to_owned(),
        who: "target `wasm`",
        program: crate::run::wasmtime_program_in(env),
        probe: Probe::VersionFlag {
            parse: wasmtime_version,
            min_major: Some(crate::run::WASMTIME_MIN_MAJOR),
        },
        needs: format!("wasmtime {} or later", crate::run::WASMTIME_MIN_MAJOR),
        missing_help: Some(crate::run::WASMTIME.missing_help()),
        too_old_help: Some(crate::run::WASMTIME.too_old_help()),
    }
}

fn cc_spec(env: &dyn Fn(&str) -> Option<OsString>) -> Spec {
    let program = crate::c::cc_command_in(env).0;
    Spec {
        name: "cc".to_owned(),
        who: "target `native`",
        program: program.clone(),
        probe: Probe::VersionFlag {
            parse: cc_version,
            min_major: None,
        },
        needs: "a C compiler".into(),
        missing_help: Some(crate::c::cc_unavailable_help(&program)),
        too_old_help: None,
    }
}

/// The second step of the check of the C compiler (`[cmd.doctor]`): once it starts, it compiles
/// and links [`cc_probe`] in a temporary directory with the command of a build, including
/// `EMELA_CC_ARGS`. The executable is not started. Also records `EMELA_CC_ARGS` when it is set.
fn check_cc_links(p: &mut ProgramCheck, env: &dyn Fn(&str) -> Option<OsString>) {
    p.args = env(CC_ARGS_ENV)
        .map(|v| v.to_string_lossy().into_owned())
        .filter(|v| !v.trim().is_empty());
    if p.status != Status::Ok {
        return;
    }
    let (_, args) = crate::c::cc_command_in(env);
    let Some(failure) = compile_probe(&p.executable, &args) else {
        return;
    };
    p.status = Status::Failed;
    p.message = Some(format!(
        "target `native` needs a C compiler that compiles and links against the C library, but `{}` failed: {failure}",
        p.executable.display()
    ));
    p.help = Some(
        "install the C headers and the C library of the system, or set EMELA_CC to another compiler"
            .to_owned(),
    );
}

/// Compiles and links [`cc_probe`] with `cc` and `args` in a new temporary directory, which is
/// removed afterwards. `None` when it succeeds; otherwise the first line of the standard error
/// that reports an error (the first line that is not blank when none says `error`).
fn compile_probe(cc: &Path, args: &[String]) -> Option<String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let dir = std::env::temp_dir().join(format!("emela-doctor-{}-{nanos}", std::process::id()));
    let result = (|| {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(PROBE_FILE), cc_probe()).map_err(|e| e.to_string())?;
        let output = crate::c::cc_invocation(cc, args, &dir, &[PathBuf::from(PROBE_FILE)])
            .output()
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let lines = || stderr.lines().map(str::trim).filter(|l| !l.is_empty());
        Err(lines()
            .find(|l| l.to_ascii_lowercase().contains("error"))
            .or_else(|| lines().next())
            .map_or_else(|| output.status.to_string(), str::to_owned))
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result.err()
}

const PROBE_FILE: &str = "probe.c";

/// The program the check of the C compiler compiles: the feature test macros of the runtime, and
/// every system header that the runtime includes (read from its sources, so that the two stay
/// the same), with a `main` that needs the math library and threads at link time.
fn cc_probe() -> String {
    let mut text = String::from(
        "#define _POSIX_C_SOURCE 200809L\n#if defined(__APPLE__)\n#define _DARWIN_C_SOURCE\n#endif\n",
    );
    for line in runtime_headers() {
        writeln!(text, "{line}").unwrap();
    }
    text.push_str(
        "#include <math.h>\n#include <pthread.h>\n\nint main(void) {\n    pthread_t self = pthread_self();\n    (void)self;\n    return (int)floor(0.5);\n}\n",
    );
    text
}

/// The `#include <...>` lines of the runtime's sources, each once, in order.
fn runtime_headers() -> Vec<&'static str> {
    let mut lines: Vec<&'static str> = Vec::new();
    for source in [
        emela_codegen_c::EMELA_H,
        emela_codegen_c::RUNTIME_H,
        emela_codegen_c::RUNTIME_C,
    ] {
        for line in source.lines().map(str::trim) {
            if line.starts_with("#include <") && !lines.contains(&line) {
                lines.push(line);
            }
        }
    }
    lines
}

fn git_spec() -> Spec {
    Spec {
        name: "git".to_owned(),
        who: "fetching Pomes",
        program: PathBuf::from("git"),
        probe: Probe::VersionFlag {
            parse: git_version,
            min_major: None,
        },
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
        (Target::Js, vec![js_host_spec(env)]),
        (Target::Native, vec![cc_spec(env)]),
        (Target::Wasm, vec![wasmtime_spec(env)]),
    ];
    let targets = specs
        .into_iter()
        .map(|(target, spec)| {
            let mut programs: Vec<ProgramCheck> =
                spec.iter().map(|spec| check(spec, env)).collect();
            if target == Target::Native {
                for p in &mut programs {
                    check_cc_links(p, env);
                }
            }
            TargetReport {
                target,
                required: required == Some(target),
                programs,
            }
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
    program_result(out, color, p);
    if let Some(args) = &p.args {
        // A setting, not a result, so the label is not colored.
        item(
            out,
            false,
            "",
            None,
            "args",
            &format!("{args} ({CC_ARGS_ENV})"),
        );
    }
}

fn program_result(out: &mut String, color: bool, p: &ProgramCheck) {
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
        let wasmtime = fake(
            dir.path(),
            "wasmtime",
            "#!/bin/sh\necho wasmtime 20.1.0\n",
            0o755,
        );
        let version = probe(&wasmtime, wasmtime_version).unwrap().unwrap();
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
        assert!(probe(&chatty, wasmtime_version).unwrap().is_none());
        assert!(probe(&failing, wasmtime_version).unwrap().is_none());
    }

    #[test]
    fn probe_of_a_program_that_cannot_start() {
        let dir = tempfile::tempdir().unwrap();
        let plain = fake(dir.path(), "node", "#!/bin/sh\necho v24.0.0\n", 0o644);
        let broken = fake(dir.path(), "broken", "#!/nonexistent/sh\n", 0o755);
        assert!(probe(&plain, wasmtime_version).is_err());
        assert!(probe(&broken, wasmtime_version).is_err());
    }

    // status

    fn probed(word: &str, min_major: Option<u32>) -> std::io::Result<Option<Found>> {
        Ok(Some(compare_major(number(word).unwrap(), min_major, "")))
    }

    #[test]
    fn statuses() {
        let cannot_start = Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert_eq!(status(None), Status::Missing);
        assert_eq!(status(Some(&cannot_start)), Status::Failed);
        assert_eq!(status(Some(&probed("20.1.0", Some(22)))), Status::TooOld);
        assert_eq!(status(Some(&probed("22.0.0", Some(22)))), Status::Ok);
        assert_eq!(status(Some(&probed("24.11.0", Some(22)))), Status::Ok);
    }

    #[test]
    fn a_version_that_cannot_be_read_passes() {
        assert_eq!(status(Some(&Ok(None))), Status::Ok);
        let cc = cc_version("Apple clang version 17.0.0\n").unwrap();
        assert_eq!(
            status(Some(&Ok(Some(compare_major(cc, Some(22), ""))))),
            Status::Ok
        );
    }

    #[test]
    fn a_program_without_a_minimum_passes_when_it_starts() {
        assert_eq!(status(Some(&probed("1.0", None))), Status::Ok);
        assert_eq!(status(Some(&Ok(None))), Status::Ok);
        assert_eq!(status(None), Status::Missing);
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
            ("EMELA_JS_HOST", "/opt/bun/bin/bun"),
            ("EMELA_WASMTIME", "/opt/wasmtime/wasmtime"),
            ("EMELA_CC", "/opt/my tools/clang"),
            ("EMELA_CC_ARGS", " -std=c23  -lsqlite3 "),
        ]);
        assert_eq!(
            crate::js_host::js_host_in(&env).program,
            PathBuf::from("/opt/bun/bin/bun")
        );
        assert_eq!(
            crate::run::wasmtime_program_in(&env),
            PathBuf::from("/opt/wasmtime/wasmtime")
        );
        // `[target.native.cc]`: `EMELA_CC` is one program, not split at whitespace;
        // `EMELA_CC_ARGS` is split.
        assert_eq!(
            crate::c::cc_command_in(&env),
            (
                PathBuf::from("/opt/my tools/clang"),
                vec!["-std=c23".to_owned(), "-lsqlite3".to_owned()]
            )
        );
    }

    #[test]
    fn programs_have_defaults_when_the_variables_are_unset_or_blank() {
        for env in [
            vars(&[]),
            vars(&[
                ("EMELA_JS_HOST", ""),
                ("EMELA_WASMTIME", ""),
                ("EMELA_CC", "  "),
            ]),
        ] {
            assert_eq!(
                crate::js_host::js_host_in(&env).program,
                PathBuf::from("node")
            );
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
        let node = fake(dir.path(), "node", "#!/bin/sh\necho node 24.11.0\n", 0o755);
        let env = on_path(dir.path(), &[]);
        let check = check(&js_host_spec(&env), &env);
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
        let node = fake(dir.path(), "node", "#!/bin/sh\necho node 20.1.0\n", 0o755);
        let env = on_path(dir.path(), &[]);
        let check = check(&js_host_spec(&env), &env);
        assert_eq!(check.status, Status::TooOld);
        assert_eq!(check.version.as_deref(), Some("20.1.0"));
        assert_eq!(
            check.message,
            Some(format!(
                "target `js` needs Node 22 or later, but `{}` is Node 20.1.0",
                node.display()
            ))
        );
        // The note of E0904 for a JS host that is too old.
        assert_eq!(
            check.help.as_deref(),
            Some(
                "install Node.js 22 or later or Bun 1.4.3 or later, or set EMELA_JS_HOST to a newer JS host"
            )
        );
    }

    #[test]
    fn a_bun_is_named_and_compared_with_the_minimum_of_bun() {
        let dir = tempfile::tempdir().unwrap();
        let bun = fake(dir.path(), "bun", "#!/bin/sh\necho bun 1.4.4\n", 0o755);
        let env = on_path(dir.path(), &[("EMELA_JS_HOST", "bun")]);
        let ok = check(&js_host_spec(&env), &env);
        assert_eq!(ok.name, "bun");
        assert_eq!(ok.status, Status::Ok);
        assert_eq!(ok.executable, bun);
        assert_eq!(ok.version.as_deref(), Some("1.4.4"));

        fake(dir.path(), "bun", "#!/bin/sh\necho bun 1.4.2\n", 0o755);
        let check = check(&js_host_spec(&env), &env);
        assert_eq!(check.status, Status::TooOld);
        assert_eq!(
            check.message,
            Some(format!(
                "target `js` needs Bun 1.4.3 or later, but `{}` is Bun 1.4.2",
                bun.display()
            ))
        );
    }

    #[test]
    fn the_js_host_is_started_with_the_probe_module() {
        // A host that answers `--version` like Node but writes nothing for the module has no
        // version that can be read, and passes.
        let dir = tempfile::tempdir().unwrap();
        fake(
            dir.path(),
            "node",
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo v20.1.0; fi\n",
            0o755,
        );
        let env = on_path(dir.path(), &[]);
        let check = check(&js_host_spec(&env), &env);
        assert_eq!(check.status, Status::Ok);
        assert_eq!(check.version, None);
    }

    #[test]
    fn a_missing_js_host_is_named_by_its_command() {
        let empty = tempfile::tempdir().unwrap();
        let env = on_path(empty.path(), &[("EMELA_JS_HOST", "bun")]);
        let check = check(&js_host_spec(&env), &env);
        assert_eq!(check.name, "bun");
        assert_eq!(check.status, Status::Missing);
        assert_eq!(
            check.message.as_deref(),
            Some(
                "target `js` needs Node 22 or later or Bun 1.4.3 or later, but `bun` is not on PATH"
            )
        );
        assert_eq!(
            check.help.as_deref(),
            Some(
                "install Node.js 22 or later or Bun 1.4.3 or later and add it to PATH, or set EMELA_JS_HOST to the JS host"
            )
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
        let check = check(&js_host_spec(&env), &env);
        assert_eq!(check.status, Status::Failed);
        assert_eq!(check.executable, node);
        let prefix = format!(
            "target `js` needs Node 22 or later or Bun 1.4.3 or later, but `{}` cannot be started: ",
            node.display()
        );
        let message = check.message.unwrap();
        assert!(message.starts_with(&prefix), "{message}");
        // The diagnostic for a node that cannot start has no help.
        assert_eq!(check.help, None);
    }

    /// A C compiler that reports its version, and otherwise records its arguments in `args.txt`
    /// and runs `compile` (a shell command) in the directory of the build.
    fn fake_cc(dir: &Path, compile: &str) -> PathBuf {
        let log = dir.join("args.txt");
        fake(
            dir,
            "clang",
            &format!(
                "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'Apple clang version 17.0.0 (clang-1700)'; echo 'Target: arm64'; exit 0; fi\nprintf '%s\\n' \"$@\" > '{}'\n{compile}\n",
                log.display()
            ),
            0o755,
        )
    }

    /// The check of the native target: the first step, and then [`check_cc_links`].
    fn check_native(env: &dyn Fn(&str) -> Option<OsString>) -> ProgramCheck {
        let mut p = check(&cc_spec(env), env);
        check_cc_links(&mut p, env);
        p
    }

    #[test]
    fn a_c_compiler_from_emela_cc() {
        let dir = tempfile::tempdir().unwrap();
        let cc = fake_cc(dir.path(), "touch main");
        let empty = tempfile::tempdir().unwrap();
        let env = on_path(empty.path(), &[("EMELA_CC", cc.to_str().unwrap())]);
        let check = check_native(&env);
        assert_eq!(check.name, "cc");
        assert_eq!(check.status, Status::Ok);
        assert_eq!(check.executable, cc);
        assert_eq!(
            check.version.as_deref(),
            Some("Apple clang version 17.0.0 (clang-1700)")
        );
        assert_eq!(check.args, None);
    }

    /// `[cmd.doctor]`: the C compiler compiles and links a program that includes the headers of
    /// the runtime, with the command line of a build and `EMELA_CC_ARGS` at the end. The
    /// temporary directory is removed afterwards.
    #[test]
    fn the_c_compiler_compiles_and_links_a_program() {
        let dir = tempfile::tempdir().unwrap();
        let copy = dir.path().join("probe-copy.c");
        let cc = fake_cc(
            dir.path(),
            &format!(
                "cp probe.c '{}'; pwd > '{}'",
                copy.display(),
                dir.path().join("pwd.txt").display()
            ),
        );
        let empty = tempfile::tempdir().unwrap();
        let env = on_path(
            empty.path(),
            &[
                ("EMELA_CC", cc.to_str().unwrap()),
                ("EMELA_CC_ARGS", "-std=c23 -lsqlite3"),
            ],
        );
        let check = check_native(&env);
        assert_eq!(check.status, Status::Ok);
        assert_eq!(check.args.as_deref(), Some("-std=c23 -lsqlite3"));
        let args = std::fs::read_to_string(dir.path().join("args.txt")).unwrap();
        let args: Vec<&str> = args.lines().collect();
        assert_eq!(
            &args[args.len() - 4..],
            ["probe.c", "-lm", "-std=c23", "-lsqlite3"]
        );
        assert!(
            args.contains(&"probe.c") && args.contains(&"-std=c11"),
            "{args:?}"
        );
        let probe = std::fs::read_to_string(&copy).unwrap();
        for header in [
            "<stdio.h>",
            "<pthread.h>",
            "<poll.h>",
            "<math.h>",
            "<stdint.h>",
        ] {
            assert!(probe.contains(&format!("#include {header}")), "{probe}");
        }
        let build_dir = std::fs::read_to_string(dir.path().join("pwd.txt")).unwrap();
        assert!(!Path::new(build_dir.trim()).exists(), "{build_dir}");
    }

    #[test]
    fn a_c_compiler_that_cannot_compile_against_the_c_library() {
        let dir = tempfile::tempdir().unwrap();
        let cc = fake_cc(
            dir.path(),
            "echo 'In file included from probe.c:5:' >&2; echo \"probe.c:5:10: fatal error: 'stdio.h' file not found\" >&2; echo '1 error generated.' >&2; exit 1",
        );
        let empty = tempfile::tempdir().unwrap();
        let env = on_path(empty.path(), &[("EMELA_CC", cc.to_str().unwrap())]);
        let check = check_native(&env);
        assert_eq!(check.status, Status::Failed);
        assert_eq!(
            check.message.unwrap(),
            format!(
                "target `native` needs a C compiler that compiles and links against the C library, but `{}` failed: probe.c:5:10: fatal error: 'stdio.h' file not found",
                cc.display()
            )
        );
        assert_eq!(
            check.help.as_deref(),
            Some(
                "install the C headers and the C library of the system, or set EMELA_CC to another compiler"
            )
        );
    }

    /// `[target.native.cc]`: a value of `EMELA_CC` with whitespace names one program, so it is
    /// not found, and the help is that of E0907 for it.
    #[test]
    fn a_c_compiler_with_flags_in_emela_cc() {
        let empty = tempfile::tempdir().unwrap();
        let env = on_path(empty.path(), &[("EMELA_CC", "clang -fsanitize=address")]);
        let check = check_native(&env);
        assert_eq!(check.status, Status::Missing);
        assert_eq!(
            check.help.as_deref(),
            Some("EMELA_CC names only the compiler; pass flags in EMELA_CC_ARGS")
        );
    }

    #[test]
    fn a_missing_c_compiler() {
        let empty = tempfile::tempdir().unwrap();
        let env = on_path(empty.path(), &[]);
        let check = check_native(&env);
        assert_eq!(check.status, Status::Missing);
        assert_eq!(check.executable, PathBuf::from("cc"));
        assert_eq!(
            check.message.as_deref(),
            Some("target `native` needs a C compiler, but `cc` is not on PATH")
        );
        // The help of E0907.
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
                (Target::Native, false),
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
                (Target::Native, false),
                (Target::Wasm, true)
            ]
        );
        let names: Vec<_> = report.targets[2]
            .programs
            .iter()
            .map(|p| p.name.as_str())
            .collect();
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
        fake(dir.path(), "node", "#!/bin/sh\necho node 24.11.0\n", 0o755);
        let env = on_path(dir.path(), &[]);
        // cc, wasmtime and git are missing, but only js is required.
        assert_eq!(exit_code(&report(Some(Target::Js), &env)), 0);
        assert_eq!(exit_code(&report(None, &env)), 0);
    }

    #[test]
    fn a_too_old_program_of_a_required_target_fails() {
        let dir = tempfile::tempdir().unwrap();
        fake(dir.path(), "node", "#!/bin/sh\necho node 20.1.0\n", 0o755);
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
        fake(dir.path(), "node", "#!/bin/sh\necho node 24.11.0\n", 0o755);
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
          missing  target `js` needs Node 22 or later or Bun 1.4.3 or later, but `node` is not on PATH
                   install Node.js 22 or later or Bun 1.4.3 or later and add it to PATH, or set EMELA_JS_HOST to the JS host
        native
          missing  target `native` needs a C compiler, but `cc` is not on PATH
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

    /// `[cmd.doctor]`: when `EMELA_CC_ARGS` is set, the line of the C compiler is followed by a
    /// line with its value.
    #[test]
    fn emela_cc_args_is_shown_under_the_c_compiler() {
        let p = ProgramCheck {
            name: "cc".to_owned(),
            status: Status::Ok,
            executable: PathBuf::from("/usr/bin/cc"),
            version: Some("cc 14".to_owned()),
            message: None,
            help: None,
            args: Some("-std=c23 -lsqlite3".to_owned()),
        };
        let mut out = String::new();
        program(&mut out, false, &p);
        insta::assert_snapshot!(out, @r"
          ok       cc cc 14 (/usr/bin/cc)
          args     -std=c23 -lsqlite3 (EMELA_CC_ARGS)
        ");
    }

    #[test]
    fn text_when_some_are_installed() {
        let text = some_installed(|report| render_text(report, EMELA, false));
        insta::assert_snapshot!(text, @r"
        emela 0.28.0+4e84642

        js (required)
          ok       node 24.11.0 ([DIR]/node)
        native
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
        native
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
                            "target `js` needs Node 22 or later or Bun 1.4.3 or later, but `node` is not on PATH",
                            "install Node.js 22 or later or Bun 1.4.3 or later and add it to PATH, or set EMELA_JS_HOST to the JS host",
                        )],
                    },
                    {
                        "target": "native",
                        "required": false,
                        "programs": [missing(
                            "cc",
                            "target `native` needs a C compiler, but `cc` is not on PATH",
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
