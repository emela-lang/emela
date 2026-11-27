//! Reuses the result of a check while nothing it read has changed.
//!
//! After an edit the editor asks for diagnostics, semantic tokens, inlay hints, code actions and
//! highlights of the same contents, and each one used to check the project from scratch. A check
//! reads files only through its [`FileSystem`] (the bundled core comes from the frontend), so it
//! is a function of what it read: [`Recorder`] notes every answer the file system gave, and an
//! entry is reused only if the file system gives the same answers again. The answer is then the
//! one a new check would give, so requests still see the contents at that point
//! (`[lsp.project]`), whether or not the client reports changes on disk.

use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use emela_driver::{Analysis, FileSystem, Input};
use emela_resolve::{DirEntry, SourceFs};

/// The entries kept: the inputs a session alternates between (a project and a few standalone
/// files) without holding on to every file ever opened.
const CAPACITY: usize = 8;

/// The result of checking one input.
pub(crate) struct Checked<P> {
    pub(crate) analysis: Rc<Analysis>,
    /// `None` when the check produced no program (e.g. an import cycle).
    pub(crate) program: Option<Rc<P>>,
}

impl<P> Clone for Checked<P> {
    fn clone(&self) -> Self {
        Checked {
            analysis: Rc::clone(&self.analysis),
            program: self.program.clone(),
        }
    }
}

struct Entry<P> {
    input: Input,
    reads: Vec<Read>,
    checked: Checked<P>,
}

/// The results of the last checks, most recently used first.
pub(crate) struct CheckCache<P> {
    entries: Vec<Entry<P>>,
}

impl<P> Default for CheckCache<P> {
    fn default() -> Self {
        CheckCache {
            entries: Vec::new(),
        }
    }
}

impl<P> CheckCache<P> {
    /// The result for `input`, from the cache if every read of the cached check gives the same
    /// answer on `fs` now, otherwise from `check`, which is given a file system to read through.
    pub(crate) fn get_or_check(
        &mut self,
        fs: &dyn FileSystem,
        input: &Input,
        check: impl FnOnce(&dyn FileSystem) -> (Analysis, Option<P>),
    ) -> Checked<P> {
        if let Some(i) = self.entries.iter().position(|e| &e.input == input) {
            let entry = self.entries.remove(i);
            if entry.reads.iter().all(|read| read.same_on(fs)) {
                let checked = entry.checked.clone();
                self.entries.insert(0, entry);
                return checked;
            }
        }
        let recorder = Recorder::new(fs);
        let (analysis, program) = check(&recorder);
        let checked = Checked {
            analysis: Rc::new(analysis),
            program: program.map(Rc::new),
        };
        self.entries.insert(
            0,
            Entry {
                input: input.clone(),
                reads: recorder.into_reads(),
                checked: checked.clone(),
            },
        );
        self.entries.truncate(CAPACITY);
        checked
    }
}

/// One question a check asked the file system, with its answer. Errors are kept as their
/// messages, which can end up in diagnostics.
#[derive(Debug, PartialEq, Eq)]
enum Read {
    Dir(PathBuf, Result<Vec<DirEntry>, String>),
    File(PathBuf, Result<String, String>),
    Absolute(PathBuf, Result<PathBuf, String>),
    IsFile(PathBuf, bool),
    IsDir(PathBuf, bool),
}

impl Read {
    fn same_on(&self, fs: &dyn FileSystem) -> bool {
        match self {
            Read::Dir(path, answer) => &read_dir(fs, path) == answer,
            Read::File(path, answer) => &message(fs.read_to_string(path)) == answer,
            Read::Absolute(path, answer) => &message(fs.absolute(path)) == answer,
            Read::IsFile(path, answer) => fs.is_file(path) == *answer,
            Read::IsDir(path, answer) => fs.is_dir(path) == *answer,
        }
    }
}

fn message<T>(result: io::Result<T>) -> Result<T, String> {
    result.map_err(|err| err.to_string())
}

/// Sorted, since a listing may come in any order (`SourceFs::read_dir`).
fn read_dir(fs: &dyn FileSystem, dir: &Path) -> Result<Vec<DirEntry>, String> {
    let mut entries = message(fs.read_dir(dir))?;
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// Passes reads through to a file system and notes each answer.
struct Recorder<'a> {
    fs: &'a dyn FileSystem,
    reads: RefCell<Vec<Read>>,
}

impl<'a> Recorder<'a> {
    fn new(fs: &'a dyn FileSystem) -> Self {
        Recorder {
            fs,
            reads: RefCell::new(Vec::new()),
        }
    }

    fn into_reads(self) -> Vec<Read> {
        self.reads.into_inner()
    }

    fn note(&self, read: Read) {
        self.reads.borrow_mut().push(read);
    }
}

impl SourceFs for Recorder<'_> {
    fn read_dir(&self, dir: &Path) -> io::Result<Vec<DirEntry>> {
        let result = self.fs.read_dir(dir);
        let mut answer = match &result {
            Ok(entries) => Ok(entries.clone()),
            Err(err) => Err(err.to_string()),
        };
        if let Ok(entries) = &mut answer {
            entries.sort_by(|a, b| a.name.cmp(&b.name));
        }
        self.note(Read::Dir(dir.to_owned(), answer));
        result
    }
}

impl FileSystem for Recorder<'_> {
    fn absolute(&self, path: &Path) -> io::Result<PathBuf> {
        let result = self.fs.absolute(path);
        let answer = match &result {
            Ok(absolute) => Ok(absolute.clone()),
            Err(err) => Err(err.to_string()),
        };
        self.note(Read::Absolute(path.to_owned(), answer));
        result
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        let result = self.fs.read_to_string(path);
        let answer = match &result {
            Ok(text) => Ok(text.clone()),
            Err(err) => Err(err.to_string()),
        };
        self.note(Read::File(path.to_owned(), answer));
        result
    }

    fn is_file(&self, path: &Path) -> bool {
        let answer = self.fs.is_file(path);
        self.note(Read::IsFile(path.to_owned(), answer));
        answer
    }

    fn is_dir(&self, path: &Path) -> bool {
        let answer = self.fs.is_dir(path);
        self.note(Read::IsDir(path.to_owned(), answer));
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emela_driver::MemoryFiles;

    fn input() -> Input {
        Input::resolve(
            &MemoryFiles::new([("/p/main.emel", "")]),
            Path::new("/p/main.emel"),
        )
        .unwrap()
    }

    /// Reads the entry and counts the checks.
    fn check(fs: &dyn FileSystem, count: &mut usize) -> (Analysis, Option<String>) {
        *count += 1;
        let text = fs.read_to_string(Path::new("/p/main.emel")).unwrap();
        (Analysis::default(), Some(text))
    }

    #[test]
    fn reuses_while_reads_are_unchanged() {
        let fs = MemoryFiles::new([("/p/main.emel", "a")]);
        let mut cache = CheckCache::default();
        let mut count = 0;
        let first = cache.get_or_check(&fs, &input(), |fs| check(fs, &mut count));
        let second = cache.get_or_check(&fs, &input(), |fs| check(fs, &mut count));
        assert_eq!(count, 1);
        assert!(Rc::ptr_eq(&first.analysis, &second.analysis));
    }

    #[test]
    fn checks_again_when_a_read_changes() {
        let mut cache = CheckCache::default();
        let mut count = 0;
        let fs = MemoryFiles::new([("/p/main.emel", "a")]);
        cache.get_or_check(&fs, &input(), |fs| check(fs, &mut count));
        let fs = MemoryFiles::new([("/p/main.emel", "b")]);
        let checked = cache.get_or_check(&fs, &input(), |fs| check(fs, &mut count));
        assert_eq!(count, 2);
        assert_eq!(checked.program.as_deref().map(String::as_str), Some("b"));
    }
}
