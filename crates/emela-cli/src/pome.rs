//! `emela pome` (spec appendix B): the dependency commands. The work is in emela-pome; this reads
//! the environment, finds the project and prints what changed.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use clap::Subcommand;
use emela_driver::Dependencies;
use emela_pome::fetch::Fetcher;
use emela_pome::fetch::config::Config;
use emela_pome::fetch::progress::{Phase, Progress, Step};
use emela_pome::fetch::transport::Http;
use emela_pome::project::{Outcome, PomeError, Project};
use emela_pome::{SourcePath, Version};

use crate::progress::{StatusLine, seconds};

#[derive(Subcommand)]
pub enum PomeCommand {
    /// Add a dependency: `<source path>[@<version>]`, or `github:owner/repo`. Without a version,
    /// the latest one
    Add {
        src: String,
        /// Allow it, and the Pomes it brings in, to use `@external` (`[external] allow`)
        #[arg(long)]
        allow_external: bool,
        /// Add it to `[dev-dependencies]`, for the tests under `test/` only
        #[arg(long)]
        dev: bool,
    },
    /// Remove a dependency (from `[dependencies]` or `[dev-dependencies]`)
    Remove { src: String },
    /// Make Pome.lock match Pome.toml and get the dependencies
    Install,
    /// Raise requirements to the latest version (all of them when none is given)
    Update { src: Option<String> },
    /// Print the resolved dependency tree
    List,
    /// Check the cached dependencies against the hashes in Pome.lock
    Verify,
}

pub fn pome(command: PomeCommand) -> ExitCode {
    let progress = Arc::new(CommandProgress::new(StatusLine::stderr()));
    let result = run(command, &progress);
    // The status line is erased before anything else is written.
    progress.stop();
    match result {
        Ok(Done::Changed(outcome)) => report(outcome),
        Ok(Done::List(tree)) => print!("{tree}"),
        Ok(Done::Verified(problems)) => {
            for p in &problems {
                println!("{p}");
            }
            if !problems.is_empty() {
                eprintln!(
                    "error: {} dependencies differ from Pome.lock",
                    problems.len()
                );
                return ExitCode::from(1);
            }
            println!("the cached dependencies match Pome.lock");
        }
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(1);
        }
    }
    ExitCode::SUCCESS
}

/// What a command found, printed after its progress is erased.
enum Done {
    Changed(Outcome),
    List(String),
    Verified(Vec<String>),
}

fn run(command: PomeCommand, progress: &Arc<CommandProgress>) -> Result<Done, PomeError> {
    let config = Config::from_env(|k| std::env::var(k).ok(), home()).map_err(PomeError::Usage)?;
    let fetcher = Fetcher::new(config, Http::new()).with_progress(Shared(Arc::clone(progress)));
    let mut project = Project::find(Path::new("."))?;
    Ok(match command {
        PomeCommand::Add {
            src,
            allow_external,
            dev,
        } => Done::Changed(project.add(&fetcher, &src, allow_external, dev)?),
        PomeCommand::Remove { src } => Done::Changed(project.remove(&fetcher, &src)?),
        PomeCommand::Install => Done::Changed(project.install(&fetcher)?),
        PomeCommand::Update { src } => Done::Changed(project.update(&fetcher, src.as_deref())?),
        PomeCommand::List => Done::List(project.list(&fetcher)?),
        PomeCommand::Verify => Done::Verified(project.verify(&fetcher)?),
    })
}

fn report(outcome: Outcome) {
    if !outcome.requirements.is_empty() || !outcome.allowed.is_empty() {
        println!("Pome.toml:");
        for c in &outcome.requirements {
            println!("{c}");
        }
        for path in &outcome.allowed {
            println!("+ [external] {path}");
        }
    }
    if outcome.build_list.is_empty() {
        println!("Pome.lock is up to date");
    } else {
        println!("Pome.lock:");
        for c in &outcome.build_list {
            println!("{c}");
        }
    }
    for e in &outcome.external {
        println!("@external of {}:", e.path);
        for f in &e.added {
            println!("+ {f}");
        }
        for f in &e.removed {
            println!("- {f}");
        }
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// The dependencies of the project at `base` for the build (`Pome.lock`, fetched if missing).
/// With `tests`, `[dev-dependencies]` too.
pub fn dependencies(base: &Path, tests: bool) -> Result<Dependencies, PomeError> {
    let config = Config::from_env(|k| std::env::var(k).ok(), home()).map_err(PomeError::Usage)?;
    let progress = Arc::new(CommandProgress::new(StatusLine::stderr()));
    let dependencies = emela_driver::project_dependencies(
        &emela_driver::OsFs,
        base,
        &Fetcher::new(config, Http::new()).with_progress(Shared(Arc::clone(&progress))),
        tests,
    );
    progress.stop();
    dependencies
}

/// Shows the progress of a dependency command on a [`StatusLine`]: the Pome being worked on in
/// the redrawn line, and a kept line for each Pome downloaded and for each phase that ends.
pub struct CommandProgress {
    line: StatusLine,
    phase: Mutex<PhaseState>,
}

struct PhaseState {
    started: Instant,
    total: Option<usize>,
    /// The steps started in this phase.
    steps: usize,
    /// The Pomes downloaded in this phase.
    downloaded: usize,
}

impl CommandProgress {
    pub fn new(line: StatusLine) -> CommandProgress {
        CommandProgress {
            line,
            phase: Mutex::new(PhaseState {
                started: Instant::now(),
                total: None,
                steps: 0,
                downloaded: 0,
            }),
        }
    }

    /// Erases the status line, before the command prints anything else.
    pub fn stop(&self) {
        self.line.stop();
    }

    fn state(&self) -> std::sync::MutexGuard<'_, PhaseState> {
        self.phase.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Progress for CommandProgress {
    fn start(&self, _: Phase, total: Option<usize>) {
        *self.state() = PhaseState {
            started: Instant::now(),
            total,
            steps: 0,
            downloaded: 0,
        };
    }

    fn step(&self, step: Step<'_>) {
        let count = {
            let mut state = self.state();
            state.steps += 1;
            match state.total {
                Some(total) => format!(" ({}/{total})", state.steps.min(total)),
                None => String::new(),
            }
        };
        self.line.set(match step {
            Step::Latest(path) => format!("latest versions{count} {path}"),
            Step::Manifest(path, version) => format!("resolving {path} {version}"),
            Step::Fetch(path, version) => format!("fetching{count} {path} {version}"),
        });
    }

    fn fetched(&self, path: &SourcePath, version: &Version) {
        self.state().downloaded += 1;
        self.line.keep(&format!("downloaded {path} {version}"));
    }

    fn finish(&self, phase: Phase, count: usize) {
        let (elapsed, downloaded) = {
            let state = self.state();
            (seconds(state.started.elapsed()), state.downloaded)
        };
        let pomes = |n: usize| match n {
            1 => "1 Pome".to_string(),
            n => format!("{n} Pomes"),
        };
        self.line.keep(&match phase {
            Phase::Latest => format!("latest versions: {count} checked ({elapsed})"),
            Phase::Resolve => format!("resolved {} ({elapsed})", pomes(count)),
            Phase::Fetch => format!(
                "fetched {}: {downloaded} downloaded, {} from the cache ({elapsed})",
                pomes(count),
                count.saturating_sub(downloaded)
            ),
        });
    }
}

/// Lets the fetcher own the progress while the command keeps it, to stop it before printing.
struct Shared(Arc<CommandProgress>);

impl Progress for Shared {
    fn start(&self, phase: Phase, total: Option<usize>) {
        self.0.start(phase, total);
    }
    fn step(&self, step: Step<'_>) {
        self.0.step(step);
    }
    fn fetched(&self, path: &SourcePath, version: &Version) {
        self.0.fetched(path, version);
    }
    fn finish(&self, phase: Phase, count: usize) {
        self.0.finish(phase, count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pome(path: &str, version: &str) -> (SourcePath, Version) {
        (
            SourcePath::from_input(path).unwrap(),
            Version::parse(version).unwrap(),
        )
    }

    /// The kept lines of `update`: one per download and one per phase. The redrawn lines between
    /// them are erased, so only the kept ones remain on the screen.
    #[test]
    fn update_keeps_a_line_per_download_and_phase() {
        let (line, out) = crate::progress::tests::line(200);
        let progress = CommandProgress::new(line);
        let (a, a1) = pome("github.com/acme/a", "v1.0.0");
        let (b, b2) = pome("github.com/acme/b", "v2.0.0");
        progress.start(Phase::Latest, Some(2));
        progress.step(Step::Latest(&a));
        progress.step(Step::Latest(&b));
        progress.finish(Phase::Latest, 2);
        progress.start(Phase::Resolve, None);
        progress.step(Step::Manifest(&a, &a1));
        progress.finish(Phase::Resolve, 2);
        progress.start(Phase::Fetch, Some(2));
        progress.step(Step::Fetch(&a, &a1));
        progress.step(Step::Fetch(&b, &b2));
        progress.fetched(&b, &b2);
        progress.finish(Phase::Fetch, 2);
        progress.stop();

        let text = out.text();
        let kept: Vec<&str> = text
            .split('\n')
            .filter_map(|l| l.rsplit("\r\x1b[K").next())
            .filter(|l| !l.is_empty())
            .map(|l| l.split(" (").next().unwrap())
            .collect();
        assert_eq!(
            kept,
            [
                "latest versions: 2 checked",
                "resolved 2 Pomes",
                "downloaded github.com/acme/b v2.0.0",
                "fetched 2 Pomes: 1 downloaded, 1 from the cache",
            ]
        );
        assert!(
            text.contains("⠋ latest versions (2/2) github.com/acme/b")
                && text.contains("⠋ resolving github.com/acme/a v1.0.0")
                && text.contains("⠋ fetching (1/2) github.com/acme/a v1.0.0"),
            "{text:?}"
        );
    }
}
