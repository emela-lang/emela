//! What a [`Fetcher`](super::Fetcher) and the commands of [`Project`](crate::project::Project)
//! are doing, for a command line that shows progress. The default does nothing, so the language
//! server and the browser build are unaffected. The words shown are the caller's to choose.

use crate::source_path::SourcePath;
use crate::version::Version;

/// A stage of a dependency command, in the order they run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Asking for the latest version of each requirement (`update`, and `add` without a version).
    Latest,
    /// Walking the dependency graph by reading each version's `Pome.toml`.
    Resolve,
    /// Getting the content of every Pome in the new lock, from the cache or the sources.
    Fetch,
}

/// One unit of work inside a phase, reported before it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step<'a> {
    Latest(&'a SourcePath),
    Manifest(&'a SourcePath, &'a Version),
    Fetch(&'a SourcePath, &'a Version),
}

/// Receives the progress of the work. Every method has an empty default.
pub trait Progress {
    /// A phase starts. `total` is the number of steps when known in advance.
    fn start(&self, phase: Phase, total: Option<usize>) {
        let _ = (phase, total);
    }

    /// A step starts.
    fn step(&self, step: Step<'_>) {
        let _ = step;
    }

    /// The content of a version was got from a source (not the cache) and stored in the cache.
    fn fetched(&self, path: &SourcePath, version: &Version) {
        let _ = (path, version);
    }

    /// A phase ended without an error. `count` is the number of Pomes it covered: the
    /// requirements asked about for [`Phase::Latest`], and the Pomes of the new lock for
    /// [`Phase::Resolve`] and [`Phase::Fetch`]. A phase that fails is not finished.
    fn finish(&self, phase: Phase, count: usize) {
        let _ = (phase, count);
    }
}

/// Reports nothing.
pub struct Silent;

impl Progress for Silent {}
