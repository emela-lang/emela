//! Module names (`Http.Client`): a sequence of type-name segments.

use std::fmt;

use smol_str::SmolStr;

/// Each segment matches the lexical class of type names.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleName(Vec<SmolStr>);

impl ModuleName {
    pub fn new(segments: impl IntoIterator<Item = impl Into<SmolStr>>) -> Self {
        ModuleName(segments.into_iter().map(Into::into).collect())
    }

    /// Does not validate; checking the names in an import is the parser's job.
    pub fn parse(text: &str) -> Self {
        ModuleName::new(text.split('.'))
    }

    pub fn segments(&self) -> &[SmolStr] {
        &self.0
    }

    /// The name that `import A.B` brings into scope.
    pub fn last(&self) -> &SmolStr {
        self.0.last().expect("a module name is never empty")
    }
}

impl fmt::Display for ModuleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, segment) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(".")?;
            }
            f.write_str(segment)?;
        }
        Ok(())
    }
}
