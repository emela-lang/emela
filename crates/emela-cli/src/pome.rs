//! `emela pome` (spec appendix B): the dependency commands. The work is in emela-pome; this reads
//! the environment, finds the project and prints what changed.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Subcommand;
use emela_pome::fetch::Fetcher;
use emela_pome::fetch::config::Config;
use emela_pome::fetch::transport::Http;
use emela_pome::project::{Outcome, PomeError, Project};

#[derive(Subcommand)]
pub enum PomeCommand {
    /// Add a dependency: `<source path>[@<version>]`, or `github:owner/repo`. Without a version,
    /// the latest one
    Add { src: String },
    /// Remove a dependency
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
    match run(command) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

fn run(command: PomeCommand) -> Result<ExitCode, PomeError> {
    let config = Config::from_env(|k| std::env::var(k).ok(), home()).map_err(PomeError::Usage)?;
    let fetcher = Fetcher::new(config, Http::new());
    let mut project = Project::find(Path::new("."))?;
    match command {
        PomeCommand::Add { src } => report(project.add(&fetcher, &src)?),
        PomeCommand::Remove { src } => report(project.remove(&fetcher, &src)?),
        PomeCommand::Install => report(project.install(&fetcher)?),
        PomeCommand::Update { src } => report(project.update(&fetcher, src.as_deref())?),
        PomeCommand::List => print!("{}", project.list(&fetcher)?),
        PomeCommand::Verify => {
            let problems = project.verify(&fetcher)?;
            for p in &problems {
                println!("{p}");
            }
            if !problems.is_empty() {
                eprintln!(
                    "error: {} dependencies differ from Pome.lock",
                    problems.len()
                );
                return Ok(ExitCode::from(1));
            }
            println!("the cached dependencies match Pome.lock");
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn report(outcome: Outcome) {
    if !outcome.requirements.is_empty() {
        println!("Pome.toml:");
        for c in &outcome.requirements {
            println!("{c}");
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
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}
