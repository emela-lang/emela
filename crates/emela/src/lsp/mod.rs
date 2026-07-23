//! The Emela language server (spec 0033), started by `emela lsp`.
//!
//! Speaks LSP over stdio: diagnostics cover every error the frontend can emit
//! — collected across declarations, not just the first — and completion is
//! context-aware (import paths, `match`/`catch` variants, `uses` effects,
//! `::` type paths, keywords, and in-scope names). The protocol layer is
//! hand-written on serde_json, like the rest of the CLI.

mod analysis;
mod completion;
mod documents;
mod hover;
mod position;
mod protocol;
mod rpc;
mod server;

use std::path::PathBuf;

use crate::error::Result;

/// Runs the server over stdio until the client sends `exit`. The process
/// exits 0 after an orderly `shutdown`, 1 otherwise (spec 0033).
pub(crate) fn run(
    package_paths: Vec<PathBuf>,
    platform_registry: Vec<emela_codegen::PlatformFn>,
) -> Result<()> {
    let code = server::run(package_paths, platform_registry)?;
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}
