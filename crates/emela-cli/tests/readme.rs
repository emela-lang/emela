//! The Status section of the README names the latest release recorded in `CHANGELOG.md`.

use std::path::Path;

fn read(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The version of the first released entry (`## [x.y.z](...)`) of `CHANGELOG.md`.
fn latest_release() -> String {
    read("CHANGELOG.md")
        .lines()
        .filter_map(|line| line.strip_prefix("## ["))
        .filter_map(|rest| rest.split_once(']').map(|(version, _)| version))
        .find(|version| *version != "Unreleased")
        .expect("CHANGELOG.md has a released entry")
        .to_string()
}

/// The section of the README from `## Status` to the next `## ` heading.
fn status_section(readme: &str) -> &str {
    let start = readme
        .find("## Status\n")
        .expect("README has a Status section");
    let rest = &readme[start + "## Status\n".len()..];
    &rest[..rest.find("\n## ").unwrap_or(rest.len())]
}

#[test]
fn status_names_the_latest_release() {
    let latest = latest_release();
    let readme = read("README.md");
    let status = status_section(&readme);
    assert!(
        status.contains(&format!("The current release is **{latest}**.")),
        "the Status section of README.md does not name {latest} as the current release"
    );
}

#[test]
fn status_table_has_a_row_for_the_latest_minor_release() {
    let latest = latest_release();
    let minor = latest
        .rsplit_once('.')
        .map_or(latest.as_str(), |(minor, _)| minor);
    let readme = read("README.md");
    let labels: Vec<&str> = status_section(&readme)
        .lines()
        .filter_map(|line| line.strip_prefix('|'))
        .filter_map(|line| line.split('|').next())
        .map(str::trim)
        .collect();
    assert!(
        labels.contains(&minor),
        "the Status table of README.md has no row for {minor} (rows: {labels:?})"
    );
}
