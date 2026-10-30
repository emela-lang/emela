//! Language service operations, independent of concrete providers.
use crate::contract::ir::TypeId;
#[derive(Clone)]
pub(crate) struct Operation {
    pub(crate) peripheral: String,
    pub(crate) name: String,
    pub(crate) params: Vec<TypeId>,
    pub(crate) ret: TypeId,
    pub(crate) fails: bool,
}
