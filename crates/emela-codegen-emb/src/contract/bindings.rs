//! Generated-value external call ABI, independent of adapters and platform code.
use crate::contract::{ExternalResult, ir::FnId};
use std::collections::HashMap;
#[derive(Clone)]
pub(crate) struct ExternalBindingPlan {
    pub(crate) symbol: String,
    pub(crate) result: ExternalResult,
    pub(crate) adapted: bool,
    pub(crate) target_owned: bool,
}
pub(crate) type ExternalBindings = HashMap<FnId, ExternalBindingPlan>;
