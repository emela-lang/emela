//! "Did you mean" suggestions for undefined names.
//!
//! Picks the closest candidate whose Levenshtein distance is at most a third of the name's length
//! (at least 1). Ties go to the candidate passed first.

use smol_str::SmolStr;

/// The nearest name within a third of the input's length (at least one edit).
/// Ties preserve candidate order, so callers can prefer inner scopes.
pub fn best_match<'a>(
    name: &str,
    candidates: impl IntoIterator<Item = &'a SmolStr>,
) -> Option<SmolStr> {
    let limit = (name.chars().count() / 3).max(1);
    let mut best: Option<(usize, &SmolStr)> = None;
    for candidate in candidates {
        if candidate == name {
            continue;
        }
        let distance = levenshtein(name, candidate);
        if distance <= limit && best.is_none_or(|(d, _)| distance < d) {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, c)| c.clone())
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            let next = (row[j + 1] + 1).min(row[j] + 1).min(prev + cost);
            prev = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 距離() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", "abc"), 0);
    }

    #[test]
    fn 近い名前を選ぶ() {
        let names: Vec<SmolStr> = ["count", "counter", "map"].map(Into::into).to_vec();
        assert_eq!(best_match("cout", &names).as_deref(), Some("count"));
        assert_eq!(best_match("mpa", &names), None);
        assert_eq!(best_match("zzz", &names), None);
    }
}
