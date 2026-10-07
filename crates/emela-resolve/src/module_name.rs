//! モジュール名（`Http.Client`）．型名の区切りの列．

use std::fmt;

use smol_str::SmolStr;

/// `.` で区切ったモジュール名．各区切りは型名の字句クラスに合う．
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleName(Vec<SmolStr>);

impl ModuleName {
    pub fn new(segments: impl IntoIterator<Item = impl Into<SmolStr>>) -> Self {
        ModuleName(segments.into_iter().map(Into::into).collect())
    }

    /// `Http.Client` を区切る．検査はしない（import 側の名前の検査は構文の仕事）．
    pub fn parse(text: &str) -> Self {
        ModuleName::new(text.split('.'))
    }

    pub fn segments(&self) -> &[SmolStr] {
        &self.0
    }

    /// `import A.B` でスコープに入る最後の名前．
    pub fn last(&self) -> &SmolStr {
        self.0.last().expect("モジュール名は空にならない")
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
