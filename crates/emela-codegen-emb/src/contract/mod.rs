//! Data contracts. These modules must not depend on compiler or platform implementations.

mod error;
mod options;
mod output;

pub use error::{Error, ErrorKind};
pub use options::{ExternalAdapter, ExternalResult, Options};
pub use output::{ExportedFunction, GeneratedFile, Output};

mod derived;
pub(crate) mod ir;
pub use derived::DerivedOperation;
pub(crate) mod abi;
pub(crate) mod auxiliary;
pub(crate) mod bindings;
pub(crate) mod control;
pub(crate) mod lowered;
pub(crate) mod operations;
pub(crate) mod planned;
pub(crate) mod runtime;
pub(crate) mod strings;

pub(crate) mod services;

#[cfg(test)]
pub(crate) mod fixtures;

pub(crate) mod artifact;
