use std::env;
use std::path::PathBuf;

use crate::package::manifest::GitDependency;

pub(crate) fn cache_root() -> PathBuf {
    if let Some(home) = env::var_os("EMELA_HOME") {
        return PathBuf::from(home).join("cache");
    }
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join(".emela").join("cache");
    }
    PathBuf::from(".emela").join("cache")
}

pub(crate) fn git_cache_path(dependency: &GitDependency) -> PathBuf {
    cache_root()
        .join("git")
        .join(sanitize_git_url(&dependency.git))
        .join(&dependency.rev)
}

pub(crate) fn sanitize_git_url(url: &str) -> String {
    let mut sanitized = String::new();
    for ch in url.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
            sanitized.push(ch);
        } else {
            sanitized.push('_');
        }
    }
    sanitized.trim_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::sanitize_git_url;

    #[test]
    fn sanitizes_git_url_for_cache_path() {
        assert_eq!(
            sanitize_git_url("https://github.com/emela-lang/stdlib.git"),
            "https___github.com_emela-lang_stdlib.git"
        );
    }
}
