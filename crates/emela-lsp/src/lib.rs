//! The language server.
//!
//! Analysis is left to `emela_driver`; this crate only converts to and from LSP messages. Unsaved
//! contents of open files are overlaid on the file system, so diagnostics match `emela check` and
//! formatting matches `emela fmt`.

mod convert;
mod overlay;
mod server;

pub use convert::{SOURCE, path_to_uri, position, uri_to_path};
pub use overlay::Overlay;
pub use server::{Error, capabilities, serve};

use emela_driver::{Frontend, OsFs};
use lsp_server::Connection;

/// Runs the server over stdio (`emela lsp`). Returns after receiving `exit`.
pub fn run_stdio<F: Frontend>(frontend: impl FnMut() -> F) -> Result<(), Error> {
    let (connection, io_threads) = Connection::stdio();
    serve(&connection, OsFs, frontend)?;
    drop(connection);
    io_threads.join()?;
    Ok(())
}
