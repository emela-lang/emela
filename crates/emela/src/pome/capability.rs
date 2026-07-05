//! Capability auditing for Pome fetches (spec 0032 CAP1-CAP3).
//!
//! Emela's defining property in a decentralized distribution model: the set of
//! capabilities a Pome requires can be *computed from its source* (0025), so
//! `emela pome add` need not trust any registry's self-report (CAP1). This
//! module walks the fetched source of a Pome and its transitive dependencies and
//! unions the capability effects their declarations carry.
//!
//! The set is derived from declared effect rows: every platform function
//! (`extern fn`, spec 0013) must declare `uses { <capability> }`, and ordinary
//! functions declare the effects they perform via `uses { ... }`. Their union is
//! what this Pome would require. This presentation is an audit aid, not the
//! enforcement point — the sandbox remains the final authority (CAP3).

use std::collections::BTreeSet;
use std::path::Path;

use crate::error::Result;
use crate::parser::parse_program;

/// Computes the union of capability effects declared across the `.emel` source
/// under each directory in `roots` (a Pome's checkout plus its transitive
/// dependencies' checkouts). Files that fail to parse are skipped so a single
/// unreadable file does not abort the audit; the result is best-effort by
/// design (CAP being a SHOULD, with the sandbox as the true gate, CAP3).
pub(crate) fn required_capabilities(roots: &[std::path::PathBuf]) -> Result<BTreeSet<String>> {
    let mut capabilities = BTreeSet::new();
    for root in roots {
        for file in emel_files(root) {
            let Ok(source) = std::fs::read_to_string(&file) else {
                continue;
            };
            let label = file.display().to_string();
            // A file that doesn't fully parse is skipped, as before multi-error
            // collection (spec 0033): CAP is a SHOULD, the sandbox is the gate.
            let (program, errors) = parse_program(&label, &source);
            if !errors.is_empty() {
                continue;
            }
            for function in &program.functions {
                capabilities.extend(function.effects.effects.iter().cloned());
            }
            for declaration in &program.externs {
                capabilities.extend(declaration.effects.effects.iter().cloned());
            }
        }
    }
    Ok(capabilities)
}

/// All `.emel` files under `root`, excluding a `.git` directory. A small manual
/// walk keeps the dependency surface at zero (no `walkdir`).
fn emel_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().and_then(|n| n.to_str()) == Some(".git") {
                    continue;
                }
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("emel") {
                out.push(path);
            }
        }
    }
    // Deterministic order (the set result is order-independent, but this keeps
    // any future diagnostics stable).
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("emela-cap-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn unions_declared_effects() {
        let dir = scratch("union");
        std::fs::write(
            dir.join("net.emel"),
            "module net\npub fn get(url: String) -> String uses { net } {\n  url\n}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("clock.emel"),
            "module clock\npub fn now() -> Int uses { clock } {\n  0\n}\n",
        )
        .unwrap();
        let caps = required_capabilities(std::slice::from_ref(&dir)).unwrap();
        assert!(caps.contains("net"));
        assert!(caps.contains("clock"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skips_git_directory() {
        let dir = scratch("skipgit");
        let git = dir.join(".git");
        std::fs::create_dir_all(&git).unwrap();
        std::fs::write(
            git.join("hook.emel"),
            "module h\npub fn f() -> Int uses { fs } {\n  0\n}\n",
        )
        .unwrap();
        let caps = required_capabilities(std::slice::from_ref(&dir)).unwrap();
        assert!(!caps.contains("fs"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
