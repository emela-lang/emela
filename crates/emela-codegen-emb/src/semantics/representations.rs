use crate::contract::ir::Repr;
pub(crate) fn i32_value(repr: Repr) -> &'static str {
    let Repr::I32 = repr else {
        unreachable!("variant dispatch")
    };
    "int32_t"
}
pub(crate) fn i64_value(repr: Repr) -> &'static str {
    let Repr::I64 = repr else {
        unreachable!("variant dispatch")
    };
    "int64_t"
}
pub(crate) fn f64_value(repr: Repr) -> &'static str {
    let Repr::F64 = repr else {
        unreachable!("variant dispatch")
    };
    "double"
}
pub(crate) fn ref_value(repr: Repr) -> &'static str {
    let Repr::Ref = repr else {
        unreachable!("variant dispatch")
    };
    "void *"
}
pub(crate) fn void_value(repr: Repr) -> &'static str {
    let Repr::Void = repr else {
        unreachable!("variant dispatch")
    };
    "void"
}
pub(crate) fn dispatch(repr: Repr) -> &'static str {
    match repr {
        Repr::I32 => i32_value(repr),
        Repr::I64 => i64_value(repr),
        Repr::F64 => f64_value(repr),
        Repr::Ref => ref_value(repr),
        Repr::Void => void_value(repr),
    }
}
