//! Fixed ordered runtime helper requirements.
use std::collections::BTreeSet;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RuntimeHelper {
    StringComparison,
    I32FromBits,
    I32Shift,
    NegativeShift,
    I32Division,
    I64FromBits,
    I64Shift,
    I64Division,
    FloatToInt,
    DynamicDefect,
    StringMatchesAt,
    StringContains,
    StringStartsWith,
    StringEndsWith,
    Utf8Decode,
    Grapheme,
    StringTrim,
    StringJoin,
    StringAppend,
    StringShow,
    IntToString,
    IntParse,
    FloatParse,
    FloatToString,
    StringOutput,
}
#[derive(Default)]
pub(crate) struct RuntimeRequirements {
    pub(crate) helpers: BTreeSet<RuntimeHelper>,
    pub(crate) defects: bool,
    pub(crate) debug: bool,
}
