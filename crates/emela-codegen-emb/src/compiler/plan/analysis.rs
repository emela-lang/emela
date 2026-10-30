use super::*;
pub(in crate::compiler::plan) struct HeapContext<'a, 'ir> {
    pub layouts: &'a TypeLayouts<'ir>,
    pub function: &'a Function,
    pub string_outputs: &'a HashSet<FnId>,
    pub known_empty: &'a HashSet<VarId>,
}
pub(in crate::compiler::plan) trait StringBehaviorRead {
    fn accepts(self, layouts: &TypeLayouts<'_>, ty: TypeId) -> bool;
}
impl StringBehaviorRead for StringBehavior {
    fn accepts(self, layouts: &TypeLayouts<'_>, ty: TypeId) -> bool {
        match self.result {
            BufferResult::String => matches!(layouts.shapes.types[ty], Type::String),
            BufferResult::OptionalString => option_payload_type(layouts, ty)
                .is_some_and(|ty| matches!(layouts.shapes.types[ty], Type::String)),
            BufferResult::Any => true,
        }
    }
}
