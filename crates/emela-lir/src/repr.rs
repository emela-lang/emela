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
    /// A value aggregate held by value, not on the heap: an enum with fields or a tuple whose
    /// fields are all scalars or value aggregates, and that does not contain itself. Its layout is
    /// given by the variable's Core type ([`value_layout`]). Only in modules made with value types
    /// on ([`crate::Module::value_types`], the embedded C target). It holds no references, so the
    /// reference counting pass leaves it alone.
    Agg,
}

impl Repr {
    pub fn name(self) -> &'static str {
        match self {
            Repr::I32 => "i32",
            Repr::I64 => "i64",
            Repr::F64 => "f64",
            Repr::Ref => "ref",
            Repr::Void => "void",
            Repr::Agg => "agg",
        }
    }
}

impl std::fmt::Display for Repr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The representation of a value of the Core type, without value types (see [`repr_in`]).
///
/// - Int, Bool → `i32`; Int64 → `i64`; Float → `f64`; `()`, Never → none
/// - String, List, tuples, functions, handlers, evidence tables, cells, fiber scopes, Any → `ref`
/// - An enum whose constructors all take no arguments → `i32` (the tag); other enums → `ref`.
///   Error enums are always `ref` (errors of different enums must be told apart at run time), and
///   so are `@external type`s (host handles, `EnumDef::external`) and other enums without
///   constructors.
///
/// A type parameter is not expected after monomorphisation; it is treated as `ref` (boxed).
pub fn repr_of(core: &emela_core::Module, ty: TypeId) -> Repr {
    repr_in(core, ty, false)
}

/// The representation of a value of the Core type. With `value_types`, an enum with fields or a
/// tuple that [`is_value_type`] is [`Repr::Agg`] instead of `ref`; otherwise as [`repr_of`].
pub fn repr_in(core: &emela_core::Module, ty: TypeId, value_types: bool) -> Repr {
    repr_env(core, ty, None, value_types)
}

/// Type arguments for the type parameters of the type being looked at: `Param(i)` is `args[i]`,
/// itself a type in `parent`.
#[derive(Clone, Copy)]
struct Env<'a> {
    args: &'a [TypeId],
    parent: Option<&'a Env<'a>>,
}

fn repr_env(core: &emela_core::Module, ty: TypeId, env: Option<&Env>, values: bool) -> Repr {
    match core.ty(ty) {
        Type::Int | Type::Bool => Repr::I32,
        Type::Int64 => Repr::I64,
        Type::Float => Repr::F64,
        // A set type argument (`Type::Set`) is not the type of a value.
        Type::Unit | Type::Never | Type::Set(_) => Repr::Void,
        Type::Param(i) => match env {
            Some(e) if *i < e.args.len() => repr_env(core, e.args[*i], e.parent, values),
            _ => Repr::Ref,
        },
        Type::Enum(id, _) => match enum_repr(core, *id) {
            Repr::Ref if values && value_env(core, ty, env, &mut Vec::new()) => Repr::Agg,
            r => r,
        },
        Type::Tuple(_) | Type::Cell(_) if values && value_env(core, ty, env, &mut Vec::new()) => {
            Repr::Agg
        }
        // A string of bounded capacity, held by value (the embedded C target's `EmString`).
        Type::String if values => Repr::Agg,
        // With value types, `Any` is only a caught failure (fibers are not compiled): a union of
        // the program's error values (the embedded C target's `EmError`).
        Type::Any if values => Repr::Agg,
        Type::String
        | Type::Tuple(_)
        | Type::List(_)
        | Type::Fn(_)
        | Type::Handler(_)
        | Type::Evidence
        | Type::Cell(_)
        | Type::FiberScope
        | Type::Any => Repr::Ref,
    }
}

/// Whether values of the type can be held by value: scalars (Int, Bool, Int64, Float, `()`, enums
/// whose constructors take no arguments), String (of bounded capacity), host handles (`@external type`), and enums (not error
/// enums) and tuples whose fields all can, unless they contain themselves. Type parameters must be bound
/// (monomorphic types).
pub fn is_value_type(core: &emela_core::Module, ty: TypeId) -> bool {
    value_env(core, ty, None, &mut Vec::new())
}

fn value_env(
    core: &emela_core::Module,
    ty: TypeId,
    env: Option<&Env>,
    stack: &mut Vec<EnumId>,
) -> bool {
    match core.ty(ty) {
        Type::Int | Type::Bool | Type::Int64 | Type::Float | Type::Unit | Type::String => true,
        Type::Param(i) => match env {
            Some(e) if *i < e.args.len() => value_env(core, e.args[*i], e.parent, stack),
            _ => false,
        },
        Type::Tuple(ts) => ts.iter().all(|&t| value_env(core, t, env, stack)),
        // A cell (the state of `State.run`) is a pointer to a local of the function that runs
        // it: the handler that holds it cannot leave the call (spec 8.6).
        Type::Cell(t) => value_env(core, *t, env, stack),
        Type::Enum(id, args) => {
            let def = &core.enums[*id];
            // A host handle (`@external type`) is held as it is (a pointer in C).
            if def.external {
                return true;
            }
            // Error enums too: with value types, a failure is a union of error values.
            if def.variants.is_empty() {
                return false;
            }
            if enum_repr(core, *id) == Repr::I32 {
                return true;
            }
            // Conservative: an enum inside itself (even at other type arguments) is not a value.
            if stack.contains(id) {
                return false;
            }
            stack.push(*id);
            let inner = Env { args, parent: env };
            let ok = def.variants.iter().all(|v| {
                v.tys
                    .iter()
                    .all(|&t| value_env(core, t, Some(&inner), stack))
            });
            stack.pop();
            ok
        }
        Type::Never
        | Type::Set(_)
        | Type::List(_)
        | Type::Fn(_)
        | Type::Handler(_)
        | Type::Evidence
        | Type::FiberScope
        | Type::Any => false,
    }
}

/// The representation of the values of an enum without value types (see [`repr_of`]). With value
/// types, an enum with fields may be [`Repr::Agg`] depending on its type arguments ([`repr_in`]).
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
/// substituted type.
pub fn repr_of_subst(core: &emela_core::Module, ty: TypeId, args: &[TypeId]) -> Repr {
    repr_subst_in(core, ty, args, false)
}

/// [`repr_of_subst`] with or without value types.
pub fn repr_subst_in(
    core: &emela_core::Module,
    ty: TypeId,
    args: &[TypeId],
    value_types: bool,
) -> Repr {
    repr_env(core, ty, Some(&Env { args, parent: None }), value_types)
}

/// The layout of a value aggregate ([`Repr::Agg`]): the field types, with type arguments
/// substituted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layout {
    /// The fields of each constructor, in constructor order.
    Enum(Vec<Vec<TypeId>>),
    Tuple(Vec<TypeId>),
    /// A host handle (`@external type`), held as the host's pointer.
    Opaque,
    /// A cell of a value of the type: a pointer to a local holding it.
    Cell(TypeId),
    /// A String: its bytes, of bounded capacity.
    String,
    /// `Any`, a caught failure: one of the program's error values, and which.
    Failure,
}

/// The layout of a value aggregate type, interning the substituted field types.
pub fn value_layout(core: &mut emela_core::Module, ty: TypeId) -> Option<Layout> {
    if matches!(core.ty(ty), Type::Any) {
        return Some(Layout::Failure);
    }
    if !is_value_type(core, ty) {
        return None;
    }
    match core.ty(ty).clone() {
        Type::Tuple(ts) => Some(Layout::Tuple(ts)),
        Type::Enum(id, _) if core.enums[id].external => Some(Layout::Opaque),
        Type::Cell(t) => Some(Layout::Cell(t)),
        Type::String => Some(Layout::String),
        Type::Enum(id, args) if enum_repr(core, id) == Repr::Ref => {
            let variants: Vec<Vec<TypeId>> = core.enums[id]
                .variants
                .iter()
                .map(|v| v.tys.clone())
                .collect();
            Some(Layout::Enum(
                variants
                    .into_iter()
                    .map(|tys| {
                        tys.into_iter()
                            .map(|t| {
                                if args.is_empty() {
                                    t
                                } else {
                                    core.types.subst(t, &args)
                                }
                            })
                            .collect()
                    })
                    .collect(),
            ))
        }
        _ => None,
    }
}
