//! Appends the commit to the version that `emela --version` prints: `emela <version>+<short hash>`.
//!
//! The workspace version stays the same between releases, so without the commit two builds of
//! different commits print the same line. The suffix is semver build metadata: it does not take
//! part in comparisons of versions. A build outside a git checkout (a source archive, a Nix build)
//! prints no suffix.
//!
//! Uncommitted changes are not marked: Cargo reruns this script only when the files named below
//! change, so a mark of them would go stale after an edit in another crate.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-env=EMELA_VERSION_SUFFIX={}", suffix());
}

fn suffix() -> String {
    let Some(hash) = git(&["rev-parse", "--short=7", "HEAD"]) else {
        return String::new();
    };
    // Rerun after a commit or a checkout. `--git-path` resolves the files of a linked worktree as
    // well as those of the main checkout.
    for path in ["HEAD", "logs/HEAD"] {
        if let Some(p) = git(&["rev-parse", "--git-path", path]) {
            println!("cargo:rerun-if-changed={}", PathBuf::from(p).display());
        }
    }
    format!("+{hash}")
}

/// The trimmed standard output of a successful `git` command, run in this crate's directory.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
