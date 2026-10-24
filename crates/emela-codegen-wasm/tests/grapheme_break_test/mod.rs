//! The cases of the official `GraphemeBreakTest.txt` (Unicode 17.0.0).
//!
//! Shared by the unit test of the reference rules (`src/emit/runtime/unicode.rs`) and the e2e
//! test of the runtime (`tests/unicode.rs`), so the file is parsed and counted in one place.

/// Every case of the file, as the graphemes of the line in order.
pub fn cases() -> Vec<Vec<String>> {
    let text = include_str!("../../data/unicode/17.0.0/GraphemeBreakTest.txt");
    assert!(
        text.contains("GraphemeBreakTest-17.0.0.txt"),
        "the vendored test file is not Unicode 17.0.0"
    );
    let mut cases = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut graphemes = Vec::new();
        let mut current = String::new();
        for token in line.split_whitespace() {
            match token {
                // A break: the cluster so far ends. The first marker comes before any code point.
                "÷" => {
                    if !current.is_empty() {
                        graphemes.push(std::mem::take(&mut current));
                    }
                }
                "×" => {}
                hex => {
                    let cp = u32::from_str_radix(hex, 16).expect("a code point");
                    current.push(char::from_u32(cp).expect("a scalar value"));
                }
            }
        }
        assert!(current.is_empty(), "a test line ends with a break");
        cases.push(graphemes);
    }
    assert_eq!(cases.len(), 766, "the test file must have all 766 cases");
    cases
}
