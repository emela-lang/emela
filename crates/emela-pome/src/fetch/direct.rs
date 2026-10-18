//! `direct`: the repository itself, with the machine's `git` (spec 4.7). Tags come from
//! `git ls-remote --tags`; a version is a shallow fetch of its tag into a bare repository, and the
//! files are read from the git tree, not a working tree. A Pome in a directory of its repository
//! has tags prefixed with the directory, and its files are those under the directory.

use std::io::Write;
use std::process::{Command, Stdio};

use crate::fetch::FetchError;
use crate::fetch::archive::{Files, MAX_FILES, MAX_TREE_BYTES};
use crate::source_path::SourcePath;
use crate::version::Version;

/// Uses the user's git configuration as is, so that private repositories (`EMELA_POMEPRIVATE`)
/// work with the user's credentials and `url.<base>.insteadOf`.
pub struct Direct {
    /// The URL of a repository. `https://<repository>.git` by default; tests use `file://`.
    pub url: Box<dyn Fn(&str) -> String>,
}

pub struct Fetched {
    pub commit: String,
    pub files: Files,
}

impl Default for Direct {
    fn default() -> Self {
        Direct {
            url: Box::new(|repo| format!("https://{repo}.git")),
        }
    }
}

impl Direct {
    /// The tags of the repository that are versions of `path`.
    pub fn versions(&self, path: &SourcePath) -> Result<Vec<Version>, FetchError> {
        let url = (self.url)(path.repository());
        let out = self
            .git(None, &["ls-remote", "--tags", "--refs", &url], None)
            .map_err(|e| not_found_or(e, path))?;
        let mut versions: Vec<Version> = String::from_utf8_lossy(&out)
            .lines()
            .filter_map(|l| {
                l.split('\t')
                    .nth(1)?
                    .strip_prefix("refs/tags/")
                    .map(str::to_string)
            })
            .filter_map(|t| path.version_of_tag(&t))
            .collect();
        versions.sort();
        Ok(versions)
    }

    pub fn fetch(&self, path: &SourcePath, version: &Version) -> Result<Fetched, FetchError> {
        let dir = tempfile::tempdir().map_err(|e| FetchError::Cache(e.to_string()))?;
        let cwd = Some(dir.path());
        let tag = format!("refs/tags/{}", path.tag(version));
        self.git(cwd, &["init", "--quiet", "--bare", "."], None)
            .map_err(FetchError::Network)?;
        let url = (self.url)(path.repository());
        self.git(
            cwd,
            &[
                "fetch",
                "--quiet",
                "--depth=1",
                "--no-tags",
                &url,
                &format!("+{tag}:{tag}"),
            ],
            None,
        )
        .map_err(|e| not_found_or(e, path))?;
        let commit = self
            .git(
                cwd,
                &["rev-parse", "--verify", &format!("{tag}^{{commit}}")],
                None,
            )
            .map_err(FetchError::Network)?;
        let commit = String::from_utf8_lossy(&commit).trim().to_string();
        // A Pome in a directory is the tree of that directory, with paths relative to it.
        let treeish = match path.directory() {
            Some(dir) => format!("{commit}:{dir}"),
            None => commit.clone(),
        };
        let tree = self
            .git(cwd, &["ls-tree", "-r", "-l", "-z", &treeish], None)
            .map_err(|e| match path.directory() {
                Some(dir) => FetchError::NotFound(format!(
                    "{path}@{version}: no directory `{dir}` at the tag ({e})"
                )),
                None => FetchError::Network(e),
            })?;

        let mut entries = Vec::new();
        let mut total = 0u64;
        for record in tree.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let tab = record
                .iter()
                .position(|b| *b == b'\t')
                .ok_or_else(|| bad("ls-tree"))?;
            let meta = String::from_utf8_lossy(&record[..tab]);
            let name = std::str::from_utf8(&record[tab + 1..])
                .map_err(|_| refused(&format!("{path}@{version}: a path is not UTF-8")))?;
            let fields: Vec<&str> = meta.split_whitespace().collect();
            let [mode, kind, object, size] = fields.as_slice() else {
                return Err(bad("ls-tree"));
            };
            match (*mode, *kind) {
                ("120000", _) => {
                    return Err(refused(&format!(
                        "{path}@{version}: {name} is a symbolic link"
                    )));
                }
                (_, "commit") => {
                    return Err(refused(&format!("{path}@{version}: {name} is a submodule")));
                }
                (_, "blob") => {}
                _ => return Err(refused(&format!("{path}@{version}: {name} is a {kind}"))),
            }
            total += size.parse::<u64>().map_err(|_| bad("ls-tree size"))?;
            entries.push((name.to_string(), object.to_string()));
        }
        if entries.len() > MAX_FILES || total > MAX_TREE_BYTES {
            return Err(refused(&format!(
                "{path}@{version}: more than {MAX_FILES} files or {MAX_TREE_BYTES} bytes"
            )));
        }
        let input: String = entries.iter().map(|(_, o)| format!("{o}\n")).collect();
        let out = self
            .git(cwd, &["cat-file", "--batch"], Some(input.as_bytes()))
            .map_err(FetchError::Network)?;
        let mut files = Vec::with_capacity(entries.len());
        let mut at = 0;
        for (name, _) in entries {
            let nl = out[at..]
                .iter()
                .position(|b| *b == b'\n')
                .ok_or_else(|| bad("cat-file"))?
                + at;
            let header = String::from_utf8_lossy(&out[at..nl]);
            let size: usize = header
                .split(' ')
                .nth(2)
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| bad("cat-file"))?;
            files.push((name, out[nl + 1..nl + 1 + size].to_vec()));
            at = nl + 1 + size + 1;
        }
        Ok(Fetched { commit, files })
    }

    fn git(
        &self,
        cwd: Option<&std::path::Path>,
        args: &[&str],
        stdin: Option<&[u8]>,
    ) -> Result<Vec<u8>, String> {
        let mut cmd = Command::new("git");
        cmd.args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(d) = cwd {
            cmd.current_dir(d);
        }
        let mut child = cmd.spawn().map_err(|e| format!("cannot run git: {e}"))?;
        // Write stdin on another thread: `cat-file --batch` answers while it reads, and a full
        // stdout pipe would otherwise stop it reading.
        let mut pipe = child.stdin.take().expect("piped");
        let input = stdin.unwrap_or_default().to_vec();
        let writer = std::thread::spawn(move || pipe.write_all(&input));
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        writer
            .join()
            .expect("the writer does not panic")
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(out.stdout)
        } else {
            Err(format!(
                "git {}: {}",
                args[0],
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }
}

// A missing repository or tag (or a private one: https asks for credentials) means "no such
// version"; other failures are network failures.
fn not_found_or(e: String, path: &SourcePath) -> FetchError {
    let lower = e.to_lowercase();
    let missing = [
        "couldn't find remote ref",
        "not found",
        "does not appear to be a git repository",
        "could not read username",
        "authentication failed",
    ];
    if missing.iter().any(|m| lower.contains(m)) {
        FetchError::NotFound(format!("{path}: {e}"))
    } else {
        FetchError::Network(e)
    }
}

fn refused(message: &str) -> FetchError {
    FetchError::NotFound(message.to_string())
}

fn bad(what: &str) -> FetchError {
    FetchError::Network(format!("unexpected output of git {what}"))
}
