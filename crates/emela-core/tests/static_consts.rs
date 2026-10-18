//! `@static` consts (spec 3.5, 16.6): lowering marks them and keeps them even when nothing refers
//! to them, for the embedded C target, which provides a `State` starting with their values.

mod support;

use emela_core::Roots;
use support::try_lower;

#[test]
fn static_consts_are_marked_and_kept() {
    let src = "\
type Presses(n: Int)

@static
const PRESSES = Presses(n: 0)

const UNUSED = 1

fn main() { 0 }
";
    let m = try_lower(&[("main.emel", src)], Roots::Exported).unwrap_or_else(|e| panic!("{e:#?}"));
    let statics: Vec<&str> = m
        .functions
        .iter()
        .filter(|(_, f)| f.static_state)
        .map(|(_, f)| f.name.as_str())
        .collect();
    assert_eq!(statics, ["PRESSES"]);
    assert!(!m.functions.iter().any(|(_, f)| f.name == "UNUSED"));
}
