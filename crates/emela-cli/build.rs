//! Appends the commit to the version that `emela --version` prints when the binary was not built
//! from a release.
//!
//! A build of the commit tagged `v<version>` prints `emela <version>`, so release tarballs keep
//! the form that `install.sh` and the release workflows check. Any other build from a git checkout
//! prints `emela <version>+<short hash>`, so two builds that both carry the workspace version can
//! be told apart. The suffix is semver build metadata: it does not take part in comparisons of
//! versions. A build outside a git checkout (a source archive, a Nix build) prints no suffix.
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
    // Rerun after a commit, a checkout or a new tag. `--git-path` resolves the files of a linked
    // worktree as well as those of the main checkout.
    for path in ["HEAD", "logs/HEAD", "packed-refs", "refs/tags"] {
        if let Some(p) = git(&["rev-parse", "--git-path", path]) {
            println!("cargo:rerun-if-changed={}", PathBuf::from(p).display());
        }
    }
    let release_tag = format!("v{}", env!("CARGO_PKG_VERSION"));
    let tags = git(&["tag", "--points-at", "HEAD"]).unwrap_or_default();
    if tags.lines().any(|t| t == release_tag) {
        String::new()
    } else {
        format!("+{hash}")
    }
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
