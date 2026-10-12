//! layer（8.5）と，with に並べたハンドラの展開と初期化の順（8.3，14.2）．
//!
//! - layer はハンドラの束．中身はハンドラか別の layer で，書いた順に平らに展開する．
//!   互いを含む layer は E0422 で，展開では2回目に出会ったところで止める．
//! - `with H1, ..., Hn` は，layer を展開したハンドラの並び．同じエフェクトのハンドラは後ろが
//!   優先され，前のものは並びから外す（初期化しない）．
//! - 初期化の順は依存（9.4 の依存(H)）から決める．同じ with の別のエフェクトを供給するハンドラに
//!   依存するハンドラは，その後に初期化する．循環しなければ，書いた順を崩さない最初の順にする．
//! - layer の中の誤り（同じエフェクトを2つのハンドラが供給する E0421，依存の循環 E0411，
//!   互いを含む E0422）は layer の宣言で1回だけ報告する（[`check_layers`]）．with ではそれを
//!   繰り返さない．

use emela_resolve::hir::{DefId, Item, Program, Res, Span};
use la_arena::ArenaMap;
use rustc_hash::FxHashSet;
use smol_str::SmolStr;

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::row::{Row, RowElem};

/// with（か layer）に並べたハンドラの1つ．
#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub(crate) handler: DefId,
    /// 供給するエフェクト．
    pub(crate) effect: DefId,
    /// with（layer）に書いた位置．layer から展開したものは layer の名前を書いた位置．
    pub(crate) span: Span,
    /// with（layer）に書いた何番目の名前から来たか．
    pub(crate) origin: usize,
    /// その名前が layer か．
    pub(crate) from_layer: bool,
}

/// layer の中のハンドラを，書いた順に平らに並べる（handler，layer に書いた何番目の名前から来たか）．
/// 互いを含む layer は2回目に出会ったところで止める．
pub(crate) fn flatten(program: &Program, layer: DefId) -> Vec<(DefId, usize)> {
    fn go(
        program: &Program,
        layer: DefId,
        visiting: &mut Vec<DefId>,
        out: &mut Vec<DefId>,
    ) -> bool {
        if visiting.contains(&layer) {
            return false;
        }
        let Some(Item::Layer(item)) = program.item(layer) else {
            return true;
        };
        visiting.push(layer);
        for &(res, _) in &item.members {
            let Res::Def(m) = res else {
                continue;
            };
            match program.item(m) {
                Some(Item::Handler(_)) => out.push(m),
                Some(Item::Layer(_)) => {
                    go(program, m, visiting, out);
                }
                _ => {}
            }
        }
        visiting.pop();
        true
    }
    let Some(Item::Layer(item)) = program.item(layer) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (i, &(res, _)) in item.members.iter().enumerate() {
        let Res::Def(m) = res else {
            continue;
        };
        match program.item(m) {
            Some(Item::Handler(_)) => out.push((m, i)),
            Some(Item::Layer(_)) => {
                let mut inner = Vec::new();
                go(program, m, &mut vec![layer], &mut inner);
                out.extend(inner.into_iter().map(|h| (h, i)));
            }
            _ => {}
        }
    }
    out
}

/// ハンドラが実装するエフェクト．Async のハンドラ（E0413）と，エフェクトの分からないものは `None`．
pub(crate) fn handler_effect(program: &Program, handler: DefId) -> Option<DefId> {
    let Some(Item::Handler(item)) = program.item(handler) else {
        return None;
    };
    let Res::Def(effect) = item.effect else {
        return None;
    };
    (program.prelude_def("Async") != Some(effect)).then_some(effect)
}

/// with に並べた名前（ハンドラか layer）を，ハンドラの並びに展開する．同じエフェクトのハンドラは
/// 後ろを残す（8.3）．
pub(crate) fn with_entries(program: &Program, handlers: &[(Res, Span)]) -> Vec<Entry> {
    let mut all = Vec::new();
    for (origin, &(res, span)) in handlers.iter().enumerate() {
        let Res::Def(def) = res else {
            continue;
        };
        let (members, from_layer) = match program.item(def) {
            Some(Item::Handler(_)) => (vec![def], false),
            Some(Item::Layer(_)) => (
                flatten(program, def).into_iter().map(|(h, _)| h).collect(),
                true,
            ),
            _ => continue,
        };
        for handler in members {
            let Some(effect) = handler_effect(program, handler) else {
                continue;
            };
            all.push(Entry {
                handler,
                effect,
                span,
                origin,
                from_layer,
            });
        }
    }
    keep_last(all)
}

/// 同じエフェクトのハンドラは後ろのものだけを残す．
fn keep_last(entries: Vec<Entry>) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        if !entries[i + 1..]
            .iter()
            .any(|later| later.effect == e.effect)
        {
            out.push(e.clone());
        }
    }
    out
}

/// `i` が `j` に依存する: `i` の依存に `j` のエフェクトがあり，`j` が別のエフェクトを供給する．
fn depends(entries: &[Entry], deps: &[Row], i: usize, j: usize) -> bool {
    j != i
        && entries[j].effect != entries[i].effect
        && deps[i].contains(&RowElem::Tag(entries[j].effect))
}

/// 初期化の順（`entries` の添字）．依存されるハンドラが先．依存が循環すれば，循環をたどる添字を
/// `Err` で返す．
pub(crate) fn init_order(entries: &[Entry], deps: &[Row]) -> Result<Vec<usize>, Vec<usize>> {
    if let Some(cycle) = find_cycle(entries.len(), |i| {
        (0..entries.len())
            .filter(|&j| depends(entries, deps, i, j))
            .collect()
    }) {
        return Err(cycle);
    }
    // 依存がすべて並んだものから，書いた順で最初のものを選ぶ．
    let mut done = vec![false; entries.len()];
    let mut order = Vec::new();
    while order.len() < entries.len() {
        let next = (0..entries.len())
            .find(|&i| {
                !done[i] && (0..entries.len()).all(|j| done[j] || !depends(entries, deps, i, j))
            })
            .expect("循環しないので並べられる");
        done[next] = true;
        order.push(next);
    }
    Ok(order)
}

/// 依存の循環の診断（E0411）．
pub(crate) fn cycle_diagnostic(
    program: &Program,
    entries: &[Entry],
    cycle: &[usize],
) -> Diagnostic {
    let mut names: Vec<SmolStr> = cycle
        .iter()
        .map(|&i| program.defs[entries[i].handler].name.clone())
        .collect();
    names.push(names[0].clone());
    Diagnostic {
        span: Some(entries[cycle[0]].span),
        kind: DiagnosticKind::HandlerCycle { cycle: names },
    }
}

/// 展開した layer（初期化の順のハンドラと，layer の中で満たされない依存）．
#[derive(Clone, Debug, Default)]
pub struct LayerInfo {
    /// 初期化の順のハンドラ．release はこの逆順．
    pub handlers: Vec<DefId>,
    /// layer の中で満たされない依存．with の R に残る（8.5）．
    pub uses: Row,
}

/// layer の宣言を確かめ，展開した中身を返す．`handler_uses` は推論し終えたハンドラの依存．
pub(crate) fn check_layers(
    program: &Program,
    handler_uses: &ArenaMap<DefId, Row>,
    diags: &mut Vec<Diagnostic>,
) -> ArenaMap<DefId, LayerInfo> {
    let mut out = ArenaMap::default();
    let mut reported_cycles: FxHashSet<Vec<DefId>> = FxHashSet::default();
    for (layer, data) in program.defs.iter() {
        let Some(Item::Layer(item)) = program.item(layer) else {
            continue;
        };
        // 互いを含む layer（E0422）．同じ循環は1回だけ報告する．
        if let Some(cycle) = layer_cycle(program, layer) {
            let mut key = cycle.clone();
            key.sort_by_key(|d| d.into_raw());
            if reported_cycles.insert(key) {
                let mut names: Vec<SmolStr> = cycle
                    .iter()
                    .map(|&l| program.defs[l].name.clone())
                    .collect();
                names.push(names[0].clone());
                diags.push(Diagnostic {
                    span: data.span,
                    kind: DiagnosticKind::LayerCycle { cycle: names },
                });
            }
        }

        let flat = flatten(program, layer);
        let mut entries: Vec<Entry> = Vec::new();
        for (handler, origin) in flat {
            let Some(effect) = handler_effect(program, handler) else {
                continue;
            };
            let (member, span) = item.members[origin];
            let from_layer =
                matches!(member, Res::Def(m) if matches!(program.item(m), Some(Item::Layer(_))));
            // 同じエフェクトのハンドラが2つ（E0421）．入れ子の layer の中だけの重複はその layer で報告する．
            if let Some(first) = entries.iter().find(|e| e.effect == effect) {
                if !(from_layer && first.from_layer && first.origin == origin) {
                    diags.push(Diagnostic {
                        span: Some(span),
                        kind: DiagnosticKind::DuplicateLayerEffect {
                            layer: data.name.clone(),
                            effect: program.defs[effect].name.clone(),
                            first: program.defs[first.handler].name.clone(),
                            second: program.defs[handler].name.clone(),
                        },
                    });
                }
                continue;
            }
            entries.push(Entry {
                handler,
                effect,
                span,
                origin,
                from_layer,
            });
        }

        let deps: Vec<Row> = entries
            .iter()
            .map(|e| handler_uses.get(e.handler).cloned().unwrap_or_default())
            .collect();
        let order = match init_order(&entries, &deps) {
            Ok(order) => order,
            Err(cycle) => {
                // 入れ子の layer の中だけの循環はその layer で報告する．
                if !same_inner_layer(&entries, &cycle) {
                    diags.push(cycle_diagnostic(program, &entries, &cycle));
                }
                (0..entries.len()).collect()
            }
        };
        let provided: Vec<DefId> = entries.iter().map(|e| e.effect).collect();
        let mut uses = Row::empty();
        for (e, d) in entries.iter().zip(&deps) {
            // 自分と同じエフェクトへの依存は外側が満たす（8.3）．
            for elem in d.elems() {
                match elem {
                    RowElem::Tag(t) if *t != e.effect && provided.contains(t) => {}
                    elem => uses.insert(elem.clone()),
                }
            }
        }
        out.insert(
            layer,
            LayerInfo {
                handlers: order.iter().map(|&i| entries[i].handler).collect(),
                uses,
            },
        );
    }
    out
}

/// 循環をたどるハンドラが，どれも同じ入れ子の layer から来たか（その layer で報告済み）．
pub(crate) fn same_inner_layer(entries: &[Entry], cycle: &[usize]) -> bool {
    let first = &entries[cycle[0]];
    first.from_layer && cycle.iter().all(|&i| entries[i].origin == first.origin)
}

/// `layer` から自分自身に戻る，layer の含む関係の循環．
fn layer_cycle(program: &Program, layer: DefId) -> Option<Vec<DefId>> {
    fn go(program: &Program, target: DefId, at: DefId, path: &mut Vec<DefId>) -> bool {
        let Some(Item::Layer(item)) = program.item(at) else {
            return false;
        };
        for &(res, _) in &item.members {
            let Res::Def(m) = res else {
                continue;
            };
            if !matches!(program.item(m), Some(Item::Layer(_))) {
                continue;
            }
            if m == target {
                return true;
            }
            if path.contains(&m) {
                continue;
            }
            path.push(m);
            if go(program, target, m, path) {
                return true;
            }
            path.pop();
        }
        false
    }
    let mut path = vec![layer];
    go(program, layer, layer, &mut path).then_some(path)
}

/// 有向グラフの循環を1つ探す．循環をたどる頂点の並び（最初の頂点は1回だけ）．
pub(crate) fn find_cycle(n: usize, edges: impl Fn(usize) -> Vec<usize>) -> Option<Vec<usize>> {
    // 0: 未訪問，1: 訪問中，2: 済み．
    fn visit(
        v: usize,
        edges: &dyn Fn(usize) -> Vec<usize>,
        state: &mut [u8],
        path: &mut Vec<usize>,
    ) -> Option<Vec<usize>> {
        state[v] = 1;
        path.push(v);
        for w in edges(v) {
            if state[w] == 1 {
                let start = path
                    .iter()
                    .position(|&p| p == w)
                    .expect("訪問中の頂点は道にある");
                return Some(path[start..].to_vec());
            }
            if state[w] == 0
                && let Some(cycle) = visit(w, edges, state, path)
            {
                return Some(cycle);
            }
        }
        path.pop();
        state[v] = 2;
        None
    }
    let mut state = vec![0u8; n];
    for v in 0..n {
        if state[v] == 0
            && let Some(cycle) = visit(v, &edges, &mut state, &mut Vec::new())
        {
            return Some(cycle);
        }
    }
    None
}
