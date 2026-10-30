//! Plans for const bridges and finite structural operations.
use crate::contract::ir::{FnId, TypeId};
use std::collections::{HashMap, HashSet};
#[derive(Default)]
pub(crate) struct ConstBridgePlan {
    pub(crate) getters: HashMap<FnId, FnId>,
    pub(crate) pairs: Vec<(TypeId, TypeId)>,
    pub(crate) ids: HashMap<(TypeId, TypeId), usize>,
    pub(crate) callables: Vec<(FnId, FnId, TypeId, TypeId)>,
}
#[derive(Default)]
pub(crate) struct DerivationPlan {
    pub(crate) show: bool,
    pub(crate) eq: bool,
    pub(crate) types: Vec<TypeId>,
    pub(crate) show_types: HashSet<TypeId>,
    pub(crate) eq_types: HashSet<TypeId>,
}
