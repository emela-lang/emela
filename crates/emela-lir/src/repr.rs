//! Representation types: how a value of a Core type is held by the backends.

use emela_core::{EnumId, Type, TypeId};

/// How a value is held. Memory layout (field offsets, pointer width) is not decided here; that is
/// the backend's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Repr {
    /// A 32-bit integer: Int, Bool, and the tag of an enum whose constructors take no arguments.
    I32,
    I64,
    F64,
    /// A reference to a heap value or a host value.
    Ref,
    /// No value: `()` and `Never`. No variable has it; a function returning it returns nothing.
    Void,
}

impl Repr {
    pub fn name(self) -> &'static str {
        match self {
            Repr::I32 => "i32",
            Repr::I64 => "i64",
            Repr::F64 => "f64",
            Repr::Ref => "ref",
            Repr::Void => "void",
        }
    }
}

impl std::fmt::Display for Repr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The representation of a value of the Core type.
///
/// - Int, Bool → `i32`; Int64 → `i64`; Float → `f64`; `()`, Never → none
/// - String, Bytes, List, tuples, functions, handlers, evidence tables, cells, fiber scopes, Any → `ref`
/// - An enum whose constructors all take no arguments → `i32` (the tag); other enums → `ref`.
///   Error enums are always `ref` (errors of different enums must be told apart at run time), and
///   so are `@external type`s (host handles, `EnumDef::external`) and other enums without
///   constructors.
///
/// A type parameter is not expected after monomorphisation; it is treated as `ref` (boxed).
pub fn repr_of(core: &emela_core::Module, ty: TypeId) -> Repr {
    match core.ty(ty) {
        Type::Int | Type::Bool => Repr::I32,
        Type::Int64 => Repr::I64,
        Type::Float => Repr::F64,
        // A set type argument (`Type::Set`) is not the type of a value.
        Type::Unit | Type::Never | Type::Set(_) => Repr::Void,
        Type::Enum(id, _) => enum_repr(core, *id),
        Type::String
        | Type::Bytes
        | Type::Tuple(_)
        | Type::List(_)
        | Type::Fn(_)
        | Type::Handler(_)
        | Type::Evidence
        | Type::Cell(_)
        | Type::FiberScope
        | Type::Any
        | Type::Param(_) => Repr::Ref,
    }
}

/// The representation of the values of an enum (see [`repr_of`]).
pub fn enum_repr(core: &emela_core::Module, id: EnumId) -> Repr {
    let def = &core.enums[id];
    if !def.error
        && !def.external
        && !def.variants.is_empty()
        && def.variants.iter().all(|v| v.fields.is_empty())
    {
        Repr::I32
    } else {
        Repr::Ref
    }
}

/// The representation of `ty` with `Type::Param(i)` replaced by `args[i]`, without interning the
/// substituted type: only the outermost constructor decides the representation.
pub fn repr_of_subst(core: &emela_core::Module, ty: TypeId, args: &[TypeId]) -> Repr {
    match core.ty(ty) {
        Type::Param(i) if *i < args.len() => repr_of(core, args[*i]),
        _ => repr_of(core, ty),
    }
}
