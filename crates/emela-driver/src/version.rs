//! The version of a program that a command starts (the JS host, wasmtime, cc), as it reports it.

pub(crate) struct Version {
    pub(crate) text: String,
    pub(crate) major: Option<u32>,
    pub(crate) minor: Option<u32>,
    pub(crate) patch: Option<u32>,
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
