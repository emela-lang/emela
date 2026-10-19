//! The import graph: resolves import targets, finds cycles, and returns the dependency order.
//!
//! Cycles are found with Tarjan's strongly connected components. One diagnostic is reported per
//! component, so every cycle is reported.

use std::collections::VecDeque;

use la_arena::ArenaMap;
use rowan::TextRange;

use crate::ModuleName;
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::source::{ModuleId, ModuleMap};

/// For `import A.B.{x, Y}`, `path` is `A.B`. The list of imported names is not handled here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub path: ModuleName,
    pub range: TextRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportEdge {
    pub target: ModuleId,
    pub range: TextRange,
}

#[derive(Debug, Clone, Default)]
pub struct ImportGraph {
    edges: ArenaMap<ModuleId, Vec<ImportEdge>>,
    order: Option<Vec<ModuleId>>,
}

impl ImportGraph {
    /// In source order.
    pub fn imports(&self, module: ModuleId) -> &[ImportEdge] {
        self.edges.get(module).map_or(&[], Vec::as_slice)
    }

    /// Dependency order (imported modules first), or `None` if there is a cycle. Used to order
    /// type checking.
    pub fn order(&self) -> Option<&[ModuleId]> {
        self.order.as_deref()
    }
}

/// Collects all diagnostics. Modules missing from `imports` are treated as importing nothing.
pub fn build_import_graph(
    modules: &ModuleMap,
    imports: &ArenaMap<ModuleId, Vec<Import>>,
) -> (ImportGraph, Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();
    let mut edges = ArenaMap::default();
    for (id, module) in modules.iter() {
        let mut resolved = Vec::new();
        for import in imports.get(id).into_iter().flatten() {
            match modules.lookup_from(id, &import.path) {
                Some(target) => resolved.push(ImportEdge {
                    target,
                    range: import.range,
                }),
                None => diagnostics.push(Diagnostic {
                    file: module.file.clone(),
                    range: Some(import.range),
                    kind: if modules.is_test_only(id, &import.path) {
                        DiagnosticKind::TestOnlyModule {
                            name: import.path.clone(),
                        }
                    } else {
                        DiagnosticKind::UndefinedModule {
                            name: import.path.clone(),
                        }
                    },
                }),
            }
        }
        edges.insert(id, resolved);
    }

    let mut graph = ImportGraph { edges, order: None };
    let components = tarjan(modules, &graph);
    let mut has_cycle = false;
    for component in &components {
        if let Some(diagnostic) = cycle_diagnostic(modules, &graph, component) {
            has_cycle = true;
            diagnostics.push(diagnostic);
        }
    }
    // Sort diagnostics by module; Tarjan's output is in dependency order, which reads poorly.
    diagnostics.sort_by_key(|d| d.file.clone());
    if !has_cycle {
        // Tarjan closes the components depended on first, so this is already dependency order.
        graph.order = Some(components.into_iter().flatten().collect());
    }
    (graph, diagnostics)
}

/// Strongly connected components in dependency order (imported modules first). Written without
/// recursion so that deep import chains cannot overflow the stack.
fn tarjan(modules: &ModuleMap, graph: &ImportGraph) -> Vec<Vec<ModuleId>> {
    #[derive(Clone, Copy)]
    struct NodeState {
        index: usize,
        lowlink: usize,
        on_stack: bool,
    }

    let mut state: ArenaMap<ModuleId, NodeState> = ArenaMap::default();
    let mut stack = Vec::new();
    let mut components = Vec::new();
    let mut next_index = 0;

    for (root, _) in modules.iter() {
        if state.contains_idx(root) {
            continue;
        }
        // (node, index of the next edge to visit)
        let mut work = vec![(root, 0)];
        while let Some(&mut (node, ref mut edge)) = work.last_mut() {
            if *edge == 0 && !state.contains_idx(node) {
                state.insert(
                    node,
                    NodeState {
                        index: next_index,
                        lowlink: next_index,
                        on_stack: true,
                    },
                );
                next_index += 1;
                stack.push(node);
            }
            let imports = graph.imports(node);
            if let Some(next) = imports.get(*edge) {
                *edge += 1;
                let target = next.target;
                match state.get(target).copied() {
                    None => work.push((target, 0)),
                    Some(t) if t.on_stack => {
                        let low = state[node].lowlink.min(t.index);
                        state[node].lowlink = low;
                    }
                    Some(_) => {}
                }
                continue;
            }
            work.pop();
            let NodeState { index, lowlink, .. } = state[node];
            if let Some(&(parent, _)) = work.last() {
                let low = state[parent].lowlink.min(lowlink);
                state[parent].lowlink = low;
            }
            if lowlink == index {
                let mut component = Vec::new();
                while let Some(member) = stack.pop() {
                    state[member].on_stack = false;
                    component.push(member);
                    if member == node {
                        break;
                    }
                }
                component.sort();
                components.push(component);
            }
        }
    }
    components
}

/// A diagnostic if the component is a cycle, located at the first module's import of the next
/// module.
///
/// When the component is not a simple cycle (it has extra edges), shows the shortest cycle
/// through the first module.
fn cycle_diagnostic(
    modules: &ModuleMap,
    graph: &ImportGraph,
    component: &[ModuleId],
) -> Option<Diagnostic> {
    let start = *component.first()?;
    let cycle = if component.len() == 1 {
        graph
            .imports(start)
            .iter()
            .any(|edge| edge.target == start)
            .then(|| vec![start])?
    } else {
        shortest_cycle(graph, component, start)
    };
    let next = *cycle.get(1).unwrap_or(&start);
    let range = graph
        .imports(start)
        .iter()
        .find(|edge| edge.target == next)
        .map(|edge| edge.range);
    Some(Diagnostic {
        file: modules[start].file.clone(),
        range,
        kind: DiagnosticKind::ImportCycle {
            modules: cycle.iter().map(|&id| modules[id].name.clone()).collect(),
        },
    })
}

/// The shortest path from `start` back to `start` within the component (starting at `start` and
/// ending just before returning).
fn shortest_cycle(graph: &ImportGraph, component: &[ModuleId], start: ModuleId) -> Vec<ModuleId> {
    let mut parent: ArenaMap<ModuleId, ModuleId> = ArenaMap::default();
    let mut queue = VecDeque::from([start]);
    while let Some(node) = queue.pop_front() {
        for edge in graph.imports(node) {
            let target = edge.target;
            if target == start {
                let mut path = vec![node];
                let mut current = node;
                while current != start {
                    current = parent[current];
                    path.push(current);
                }
                path.reverse();
                return path;
            }
            if component.binary_search(&target).is_ok() && !parent.contains_idx(target) {
                parent.insert(target, node);
                queue.push_back(target);
            }
        }
    }
    unreachable!("a strongly connected component has a path back to start")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::source::{MemoryFs, collect_modules};

    struct Fixture {
        modules: ModuleMap,
        imports: ArenaMap<ModuleId, Vec<Import>>,
    }

    /// Builds modules and imports from `(file, [imported modules])`. Each import's range is its
    /// index in the file, in steps of 10.
    fn fixture(files: &[(&str, &[&str])]) -> Fixture {
        let fs = MemoryFs::new(files.iter().map(|(file, _)| format!("src/{file}")));
        let (modules, diagnostics) = collect_modules(&fs, Path::new("src"));
        assert_eq!(diagnostics, []);
        let by_file: rustc_hash::FxHashMap<_, _> =
            modules.iter().map(|(id, m)| (m.file.clone(), id)).collect();
        let mut imports = ArenaMap::default();
        for (file, targets) in files {
            let id = by_file[&Path::new("src").join(file)];
            let list = targets
                .iter()
                .enumerate()
                .map(|(i, target)| Import {
                    path: ModuleName::parse(target),
                    range: TextRange::at((i as u32 * 10).into(), 5.into()),
                })
                .collect();
            imports.insert(id, list);
        }
        Fixture { modules, imports }
    }

    fn run(files: &[(&str, &[&str])]) -> (Option<Vec<String>>, Vec<String>) {
        let Fixture { modules, imports } = fixture(files);
        let (graph, diagnostics) = build_import_graph(&modules, &imports);
        let order = graph.order().map(|order| {
            order
                .iter()
                .map(|&id| modules[id].name.to_string())
                .collect()
        });
        let diagnostics = diagnostics
            .iter()
            .map(|d| {
                let range = d.range.map_or(String::new(), |r| format!("@{r:?}"));
                format!("{}{range}: {d}", d.file.display())
            })
            .collect();
        (order, diagnostics)
    }

    #[test]
    fn topological_order() {
        let (order, diagnostics) = run(&[
            ("main.emel", &["App", "Http.Client"]),
            ("app.emel", &["Http.Client", "Json"]),
            ("http/client.emel", &["Json", "Http.Url"]),
            ("http/url.emel", &[]),
            ("json.emel", &[]),
        ]);
        assert_eq!(diagnostics, Vec::<String>::new());
        let order = order.unwrap();
        let pos = |name: &str| order.iter().position(|m| m == name).unwrap();
        for (from, to) in [
            ("Main", "App"),
            ("Main", "Http.Client"),
            ("App", "Http.Client"),
            ("App", "Json"),
            ("Http.Client", "Json"),
            ("Http.Client", "Http.Url"),
        ] {
            assert!(pos(to) < pos(from), "{to} comes before {from}: {order:?}");
        }
        assert_eq!(order, ["Json", "Http.Url", "Http.Client", "App", "Main"]);
    }

    #[test]
    fn undefined_module() {
        let (order, diagnostics) = run(&[
            ("main.emel", &["Http.Client", "Http", "Missing"]),
            ("http/client.emel", &["Http.Missing"]),
        ]);
        assert_eq!(
            diagnostics,
            [
                "src/http/client.emel@0..5: undefined module `Http.Missing`",
                "src/main.emel@10..15: undefined module `Http`",
                "src/main.emel@20..25: undefined module `Missing`",
            ]
        );
        // Undefined imports are not edges, so there is still an order.
        assert_eq!(order.unwrap(), ["Http.Client", "Main"]);
    }

    #[test]
    fn self_import() {
        let (order, diagnostics) = run(&[("main.emel", &["Json", "Main"]), ("json.emel", &[])]);
        assert_eq!(
            diagnostics,
            ["src/main.emel@10..15: module `Main` imports itself"]
        );
        assert_eq!(order, None);
    }

    #[test]
    fn multiple_cycles() {
        let (order, diagnostics) = run(&[
            // A → B → A
            ("aa.emel", &["Bb"]),
            ("bb.emel", &["A2a"]),
            ("a2a.emel", &["Bb"]),
            // C → D → E → C, with a shortcut C → E
            ("cc.emel", &["Dd", "Ee"]),
            ("dd.emel", &["Ee"]),
            ("ee.emel", &["Cc"]),
            // Depends on the cycles but is not part of them
            ("main.emel", &["Cc", "Bb", "Free"]),
            ("free.emel", &[]),
        ]);
        assert_eq!(
            diagnostics,
            [
                "src/a2a.emel@0..5: import cycle: A2a → Bb → A2a",
                "src/cc.emel@10..15: import cycle: Cc → Ee → Cc",
            ]
        );
        assert_eq!(order, None);
    }

    #[test]
    fn long_cycle_lists_all_modules() {
        let (_, diagnostics) = run(&[
            ("one.emel", &["Two"]),
            ("two.emel", &["Three"]),
            ("three.emel", &["Four"]),
            ("four.emel", &["One"]),
        ]);
        // Module IDs are assigned in name order (Four, One, Three, Two).
        assert_eq!(
            diagnostics,
            ["src/four.emel@0..5: import cycle: Four → One → Two → Three → Four"]
        );
    }

    #[test]
    fn deep_chain_does_not_overflow() {
        let names: Vec<String> = (0..20_000).map(|i| format!("m{i:05}x")).collect();
        let files: Vec<(String, Vec<String>)> = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let next = names
                    .get(i + 1)
                    .map(|n| crate::naming::to_type_name(n).unwrap().to_string());
                (format!("{name}.emel"), next.into_iter().collect())
            })
            .collect();
        let files: Vec<(&str, Vec<&str>)> = files
            .iter()
            .map(|(f, t)| (f.as_str(), t.iter().map(String::as_str).collect()))
            .collect();
        let files: Vec<(&str, &[&str])> = files.iter().map(|(f, t)| (*f, t.as_slice())).collect();
        let (order, diagnostics) = run(&files);
        assert_eq!(diagnostics, Vec::<String>::new());
        let order = order.unwrap();
        assert_eq!(order.first().unwrap(), "M19999x");
        assert_eq!(order.last().unwrap(), "M00000x");
    }
}
