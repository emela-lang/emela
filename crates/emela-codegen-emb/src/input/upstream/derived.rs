//! Finite structural operations supplied by a Flow IR producer.
use std::collections::HashSet;

use emela_core::{External, ExternalBinding, HostRef, Module, Type, TypeId};
use emela_lir::{FnBuilder, Function};

use crate::{Error, ErrorKind};

pub(crate) const TARGET: &str = "emela-emb-derived";

pub use crate::contract::DerivedOperation;

/// Creates a backend-private external Flow binding for finite structural derivation.
///
/// A Flow producer adds this function to its module and calls its returned function ID. The
/// binding uses existing Flow external metadata and does not require a shared LIR extension.
/// `ty` must be a closed, valid value type from `core`; Unit and Never have no Flow parameter
/// representation. Generic enum definitions may use their declared parameters. The producer
/// chooses a unique function name. `compile_flow` verifies the complete module and signature.
/// This factory does not rewrite source Core operations or select source trait implementations.
pub fn derived_binding(
    core: &Module,
    operation: DerivedOperation,
    ty: TypeId,
    name: &str,
) -> Result<Function, Vec<Error>> {
    if !valid_type(core, ty, 0, &mut HashSet::new())
        || matches!(core.ty(ty), Type::Unit | Type::Never)
        || !supported(core, ty, operation)
    {
        return Err(vec![Error {
            kind: ErrorKind::InvalidCore,
            message: format!(
                "cannot create {operation:?} Flow derivation `{name}`: expected a closed, supported non-void value type, found {ty:?}"
            ),
        }]);
    }
    let mut builder = FnBuilder::new(core, name);
    builder.param("a", ty);
    let symbol = match operation {
        DerivedOperation::Eq => {
            builder.param("b", ty);
            builder.set_ret(TypeId::BOOL);
            "eq"
        }
        DerivedOperation::Show => {
            builder.set_ret(TypeId::STRING);
            "show"
        }
    };
    Ok(builder.finish_external(External {
        bindings: vec![ExternalBinding {
            target: TARGET.into(),
            host: HostRef::Global(symbol.into()),
        }],
    }))
}

// Validate raw generic definitions without expanding growing type applications.
fn valid_type(
    core: &Module,
    ty: TypeId,
    params: usize,
    seen: &mut HashSet<(TypeId, usize)>,
) -> bool {
    if ty.index() >= core.types.len() {
        return false;
    }
    if !seen.insert((ty, params)) {
        return true;
    }
    match core.ty(ty) {
        Type::Param(index) => *index < params,
        Type::List(element) | Type::Cell(element) => valid_type(core, *element, params, seen),
        Type::Tuple(fields) => fields
            .iter()
            .all(|&field| valid_type(core, field, params, seen)),
        Type::Enum(id, args) => {
            let Some((_, definition)) = core.enums.iter().find(|(candidate, _)| candidate == id)
            else {
                return false;
            };
            args.len() == definition.params
                && args.iter().all(|&arg| valid_type(core, arg, params, seen))
                && definition
                    .variants
                    .iter()
                    .flat_map(|variant| &variant.tys)
                    .all(|&field| valid_type(core, field, definition.params, seen))
        }
        Type::Fn(function) => {
            function
                .params
                .iter()
                .chain([&function.ret])
                .all(|&part| valid_type(core, part, params, seen))
                && function.sets.as_ref().is_none_or(|sets| {
                    [sets.errors, sets.effects]
                        .iter()
                        .all(|&set| valid_type(core, set, params, seen))
                })
        }
        Type::Set(summary) => summary
            .params
            .iter()
            .all(|&part| valid_type(core, part, params, seen)),
        _ => true,
    }
}

pub(crate) fn kind(function: &Function) -> Option<DerivedOperation> {
    let binding = function.external.as_ref()?.binding(TARGET)?;
    match &binding.host {
        HostRef::Global(symbol) if symbol == "eq" => Some(DerivedOperation::Eq),
        HostRef::Global(symbol) if symbol == "show" => Some(DerivedOperation::Show),
        _ => None,
    }
}

pub(crate) fn supported(core: &Module, ty: TypeId, kind: DerivedOperation) -> bool {
    crate::semantics::derived::supported_graph(core, ty.index(), kind)
}
/// Keeps growing Core inputs outside the convenience lowering contract until the producer
/// connection is supplied. Flow input and its finite derivation runtime are unaffected.
pub(crate) fn check_core_input(core: &Module) -> Result<(), Vec<Error>> {
    if crate::input::upstream::type_expansion::expanding_enums(core).is_empty() {
        return Ok(());
    }
    Err(vec![Error {
        kind: ErrorKind::Unsupported,
        message: "the Core convenience entry does not accept growing recursive enum families; use a Flow producer and compile_flow, supplying derived_binding functions for structural equality or display".into(),
    }])
}

impl crate::semantics::derived::TypeGraph for Module {
    fn shape(&self, ty: usize) -> crate::semantics::derived::Shape {
        use crate::semantics::derived::Shape;
        match self
            .types
            .iter()
            .nth(ty)
            .expect("type graph ID is in the input table")
        {
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
