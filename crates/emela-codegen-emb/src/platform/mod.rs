//! Connect frozen C artifacts to independently supplied platform packages.
mod contract;
pub mod execution;
mod packages;
mod plan;
pub use contract::*;
pub use packages::{board, native};
pub use plan::prepare_connection;

mod fingerprint;
