//! Minimal version selection (spec 4.7).
//!
//! Every version reachable from the root's requirements is visited, including versions that end
//! up not selected, and each Pome gets the largest version required anywhere. New tags never
//! change the result unless someone raises a requirement.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::source_path::SourcePath;
use crate::version::Version;

/// The requirements of one version of a Pome: its `[dependencies]`. Implementations read the
/// `Pome.toml` of that version (from the proxy, the cache, or a `[replace]` directory).
pub trait Requirements {
    type Error;
    fn requirements(
        &mut self,
        path: &SourcePath,
        version: &Version,
    ) -> Result<Vec<(SourcePath, Version)>, Self::Error>;
}

/// The build list: the selected version of every Pome the root depends on, directly or not. The
/// root itself is not in it.
pub fn build_list<R: Requirements>(
    root: &[(SourcePath, Version)],
    graph: &mut R,
) -> Result<BTreeMap<SourcePath, Version>, R::Error> {
    let mut selected: BTreeMap<SourcePath, Version> = BTreeMap::new();
    let mut visited: BTreeSet<(SourcePath, String)> = BTreeSet::new();
    let mut queue: VecDeque<(SourcePath, Version)> = root.iter().cloned().collect();
    while let Some((path, version)) = queue.pop_front() {
        if !visited.insert((path.clone(), version.as_str().to_string())) {
            continue;
        }
        match selected.get(&path) {
            Some(current) if *current >= version => {}
            _ => {
                selected.insert(path.clone(), version.clone());
            }
        }
        queue.extend(graph.requirements(&path, &version)?);
    }
    Ok(selected)
}

/// The Pomes of `selected` that the requirements `roots` reach: those of their own build list,
/// and those required by the selected version of a reached Pome. `selected` is a build list of a
/// larger set of requirements (`[dependencies]` with `[dev-dependencies]`, spec 4.7), so a Pome it
/// raises may require Pomes that `roots` alone would not.
pub fn reached<R: Requirements>(
    roots: &[(SourcePath, Version)],
    selected: &BTreeMap<SourcePath, Version>,
    graph: &mut R,
) -> Result<BTreeSet<SourcePath>, R::Error> {
    let mut seen: BTreeSet<SourcePath> = build_list(roots, graph)?.into_keys().collect();
    let mut queue: Vec<SourcePath> = seen.iter().cloned().collect();
    while let Some(path) = queue.pop() {
        let Some(version) = selected.get(&path) else {
            continue;
        };
        for (p, _) in graph.requirements(&path, version)? {
            if seen.insert(p.clone()) {
                queue.push(p);
            }
        }
    }
    Ok(seen)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    struct Graph(HashMap<(String, String), Vec<(SourcePath, Version)>>);

    impl Requirements for Graph {
        type Error = String;
        fn requirements(
            &mut self,
            p: &SourcePath,
            v: &Version,
        ) -> Result<Vec<(SourcePath, Version)>, String> {
            self.0
                .get(&(p.to_string(), v.to_string()))
                .cloned()
                .ok_or_else(|| format!("unknown {p}@{v}"))
        }
    }

    fn req(p: &str, v: &str) -> (SourcePath, Version) {
        (
            SourcePath::parse(&format!("example.com/{p}")).unwrap(),
            Version::parse(v).unwrap(),
        )
    }

    // The example graph: A requires B 1.2 and C 1.2; B 1.2 requires D 1.3; C 1.2 requires D 1.4;
    // C 1.2 is not the latest C, and the older D 1.3 is still visited.
    fn example() -> Graph {
        let mut g = HashMap::new();
        let mut put = |p: &str, v: &str, reqs: Vec<(SourcePath, Version)>| {
            g.insert((format!("example.com/{p}"), v.to_string()), reqs);
        };
        put("b", "v1.2.0", vec![req("d", "v1.3.0")]);
        put("c", "v1.2.0", vec![req("d", "v1.4.0")]);
        put("c", "v1.3.0", vec![req("d", "v1.5.0")]);
        put("d", "v1.3.0", vec![req("e", "v1.0.0")]);
        put("d", "v1.4.0", vec![]);
        put("e", "v1.0.0", vec![]);
        Graph(g)
    }

    #[test]
    fn selects_the_largest_required_version() {
        let list = build_list(&[req("b", "v1.2.0"), req("c", "v1.2.0")], &mut example()).unwrap();
        let got: Vec<String> = list.iter().map(|(p, v)| format!("{p}@{v}")).collect();
        // E stays: D 1.3 requires it even though D 1.4 is selected.
        assert_eq!(
            got,
            [
                "example.com/b@v1.2.0",
                "example.com/c@v1.2.0",
                "example.com/d@v1.4.0",
                "example.com/e@v1.0.0"
            ]
        );
    }

    #[test]
    fn keeps_two_majors_apart() {
        let mut g = Graph(HashMap::from([
            (("example.com/h".to_string(), "v1.4.0".to_string()), vec![]),
            (
                ("example.com/h/v2".to_string(), "v2.1.0".to_string()),
                vec![],
            ),
        ]));
        let list = build_list(&[req("h", "v1.4.0"), req("h/v2", "v2.1.0")], &mut g).unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn handles_cycles() {
        let mut g = Graph(HashMap::from([
            (
                ("example.com/a".to_string(), "v1.0.0".to_string()),
                vec![req("b", "v1.0.0")],
            ),
            (
                ("example.com/b".to_string(), "v1.0.0".to_string()),
                vec![req("a", "v1.0.0")],
            ),
        ]));
        assert_eq!(build_list(&[req("a", "v1.0.0")], &mut g).unwrap().len(), 2);
    }

    #[test]
    fn reaches_what_a_raised_version_requires() {
        // The tests require C 1.3, which requires D 1.5 and so F; B alone reaches only D 1.3 and E.
        let mut g = example();
        g.0.insert(
            ("example.com/c".into(), "v1.3.0".into()),
            vec![req("d", "v1.5.0")],
        );
        g.0.insert(
            ("example.com/d".into(), "v1.5.0".into()),
            vec![req("f", "v1.0.0")],
        );
        g.0.insert(("example.com/f".into(), "v1.0.0".into()), vec![]);
        g.0.insert(
            ("example.com/t".into(), "v1.0.0".into()),
            vec![req("c", "v1.3.0")],
        );
        let all = build_list(&[req("b", "v1.2.0"), req("t", "v1.0.0")], &mut g).unwrap();
        let built = reached(&[req("b", "v1.2.0")], &all, &mut g).unwrap();
        let got: Vec<&str> = built.iter().map(|p| p.as_str()).collect();
        assert_eq!(
            got,
            [
                "example.com/b",
                "example.com/d",
                "example.com/e",
                "example.com/f"
            ]
        );
    }
}
