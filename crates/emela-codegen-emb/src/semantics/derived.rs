//! Shared structural derivation rule. Input adapters supply only the graph view.
use crate::contract::DerivedOperation;
use std::collections::HashSet;
pub(crate) enum Shape {
    Scalar,
    Function,
    Param(usize),
    List(usize),
    Tuple(Vec<usize>),
    Enum(usize, Vec<usize>),
    Unsupported,
}
pub(crate) trait TypeGraph {
    fn shape(&self, ty: usize) -> Shape;
    fn fields(&self, enum_id: usize) -> Vec<usize>;
}
pub(crate) fn supported_graph(graph: &impl TypeGraph, ty: usize, kind: DerivedOperation) -> bool {
    fn go(
        graph: &impl TypeGraph,
        ty: usize,
        env: &[bool],
        seen: &mut HashSet<(usize, Vec<bool>)>,
        functions: bool,
    ) -> bool {
        match graph.shape(ty) {
            Shape::Scalar => true,
            Shape::Function => functions,
            Shape::Param(i) => env.get(i).copied().unwrap_or(false),
            Shape::List(t) => go(graph, t, env, seen, functions),
            Shape::Tuple(types) => types.iter().all(|&ty| go(graph, ty, env, seen, functions)),
            Shape::Enum(id, args) => {
                let flags: Vec<_> = args
                    .iter()
                    .map(|&ty| go(graph, ty, env, seen, functions))
                    .collect();
                let key = (id, flags.clone());
                if !seen.insert(key.clone()) {
                    return true;
                }
                let result = graph
                    .fields(id)
                    .iter()
                    .all(|&ty| go(graph, ty, &flags, seen, functions));
                seen.remove(&key);
                result
            }
            Shape::Unsupported => false,
        }
    }
    go(
        graph,
        ty,
        &[],
        &mut HashSet::new(),
        kind == DerivedOperation::Show,
    )
}
use crate::contract::ir::{Definitions, Type, TypeId};

impl crate::semantics::derived::TypeGraph for Definitions {
    fn shape(&self, ty: usize) -> crate::semantics::derived::Shape {
        use crate::semantics::derived::Shape;
        match self.ty(TypeId::from_index(ty)) {
            Type::Int
            | Type::Int64
            | Type::Float
            | Type::Bool
            | Type::String
            | Type::Bytes
            | Type::Unit
            | Type::Never => Shape::Scalar,
            Type::Fn(_) => Shape::Function,
            Type::Param(i) => Shape::Param(*i),
            Type::List(t) => Shape::List(t.index()),
            Type::Tuple(types) => Shape::Tuple(types.iter().map(|t| t.index()).collect()),
            Type::Enum(id, args) => Shape::Enum(
                id.into_raw().into_u32() as usize,
                args.iter().map(|t| t.index()).collect(),
            ),
            Type::Cell(_)
            | Type::Handler(_)
            | Type::Set(_)
            | Type::Evidence
            | Type::FiberScope
            | Type::Any => Shape::Unsupported,
        }
    }
    fn fields(&self, id: usize) -> Vec<usize> {
        self.enums[la_arena::Idx::from_raw(la_arena::RawIdx::from_u32(id as u32))]
            .variants
            .iter()
            .flat_map(|v| v.tys.iter().map(|t| t.index()))
            .collect()
    }
}
pub(crate) fn supported(core: &Definitions, ty: TypeId, kind: DerivedOperation) -> bool {
    supported_graph(core, ty.index(), kind)
}
