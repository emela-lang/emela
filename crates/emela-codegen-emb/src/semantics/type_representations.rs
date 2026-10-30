//! Core type representation decisions; allocation and ABI planning share TypeLayouts.
use crate::contract::{
    abi::LayoutBuilder as TypeLayouts,
    ir::{Repr, Type},
};
use crate::semantics::abi::LayoutRead;
pub(crate) fn int_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::I32
}
pub(crate) fn int64_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::I64
}
pub(crate) fn float_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::F64
}
pub(crate) fn bool_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::I32
}
pub(crate) fn string_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Ref
}
pub(crate) fn bytes_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Ref
}
pub(crate) fn unit_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Void
}
pub(crate) fn never_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Void
}
pub(crate) fn tuple_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Ref
}
pub(crate) fn list_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Ref
}
pub(crate) fn enum_repr(layouts: &TypeLayouts<'_>, ty: &Type) -> Repr {
    let Type::Enum(id, ..) = ty else {
        unreachable!("enum representation")
    };
    if layouts.enum_is_i32(*id) {
        Repr::I32
    } else {
        Repr::Ref
    }
}
pub(crate) fn param_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Ref
}
pub(crate) fn fn_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Ref
}
pub(crate) fn handler_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Ref
}
pub(crate) fn evidence_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Ref
}
pub(crate) fn cell_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Ref
}
pub(crate) fn fiber_scope_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Ref
}
pub(crate) fn any_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Ref
}
pub(crate) fn set_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Void
}
pub(crate) fn repr(layouts: &TypeLayouts<'_>, ty: &Type) -> Repr {
    match ty {
        Type::Int => int_repr(layouts, ty),
        Type::Int64 => int64_repr(layouts, ty),
        Type::Float => float_repr(layouts, ty),
        Type::Bool => bool_repr(layouts, ty),
        Type::String => string_repr(layouts, ty),
        Type::Bytes => bytes_repr(layouts, ty),
        Type::Unit => unit_repr(layouts, ty),
        Type::Never => never_repr(layouts, ty),
        Type::Tuple(..) => tuple_repr(layouts, ty),
        Type::List(..) => list_repr(layouts, ty),
        Type::Enum(..) => enum_repr(layouts, ty),
        Type::Param(..) => param_repr(layouts, ty),
        Type::Fn(..) => fn_repr(layouts, ty),
        Type::Handler(..) => handler_repr(layouts, ty),
        Type::Evidence => evidence_repr(layouts, ty),
        Type::Cell(..) => cell_repr(layouts, ty),
        Type::FiberScope => fiber_scope_repr(layouts, ty),
        Type::Any => any_repr(layouts, ty),
        Type::Set(..) => set_repr(layouts, ty),
    }
}
