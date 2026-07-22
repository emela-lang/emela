//! The Emela compiler frontend and CLI driver.
//!
//! This crate lexes, parses, resolves imports, and type-checks Emela source,
//! then lowers it to the `emela-codegen` IR and hands that IR to a selected
//! [`emela_codegen::Backend`].

mod api;
mod ast;
mod driver;
mod error;
mod external;
mod fmt;
#[cfg(feature = "run")]
mod host_abi;
#[cfg(feature = "run")]
mod http_host;
mod imports;
mod lexer;
mod lint;
mod lower;
mod lsp;
mod parser;
mod pome;
mod prelude;
#[cfg(feature = "run")]
mod random_host;
mod resolve;
#[cfg(feature = "run")]
mod run;
#[cfg(feature = "run")]
mod socket_host;
#[cfg(feature = "run")]
mod test_runner;
mod typecheck;

#[cfg(feature = "run")]
pub use api::{RunOutput, run_source};
pub use api::{check_source, compile_source, ir_source};
pub use driver::run;
pub use error::{Error, Result};

// Re-exported so embedders can name backend outputs without depending on
// `emela-codegen` directly.
pub use emela_codegen::{Artifact, ArtifactKind, EmitMode};
