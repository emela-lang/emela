//! The file-system and other `Std.FileSystem` runtime side.
//!
//! **Declared only.** The std platform effects have no `@external(wasm, ...)` bindings yet, so
//! `with Platform` is rejected during resolution (E0412) before the emitter runs. A later wave
//! will add the WASI `path_open` / `fd_read` / `fd_write` / `fd_close` wrappers here and the
//! `FileSystem` internal hosts to `emit::external::INTERNAL_HOSTS`.

use super::*;

/// The file-system fragment (currently empty).
pub(super) const FUNCS: &[Sig] = &[];

pub(super) fn body(_b: &mut Builder, _name: &str, _ins: &mut InstructionSink) -> bool {
    false
}

pub(super) fn locals(_name: &str) -> Option<Vec<(u32, ValType)>> {
    None
}
