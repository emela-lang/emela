//! Source audits deliberately have no dependency on the compiler crates.
#[allow(dead_code)]
#[path = "../../../tests/support/backend_inventory.rs"]
pub mod backend;
#[allow(dead_code)]
#[path = "../../../examples/support/feature_inventory.rs"]
pub mod definitions;
#[allow(dead_code)]
#[path = "../../../examples/support/implementation_inventory.rs"]
pub mod implementation;
