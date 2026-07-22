//! The host side of the `Random` capability (spec 0054) for `emela run`.
//!
//! The wasm backend lowers `random.raw_int` / `random.raw_bytes` to calls of
//! imported host functions in the `emela_random` module. This module implements
//! them from the OS entropy source (`getrandom`), matching the CSPRNG quality the
//! component backend obtains from `wasi:random` (spec 0054 P5; spec 0052 parity).
//! `raw_int` returns a uniform `Int`; `raw_bytes` allocates a `Bytes` value
//! (`[len][bytes]`) into guest linear memory through the exported `alloc`.
//!
//! `Random` is stateless, so — unlike `Socket` — no per-run table is held in the
//! wasmi store; each call draws fresh entropy.

use wasmi::Caller;

use crate::host_abi::{alloc_func, alloc_string, host_fail, memory};
use crate::run::Host;

/// An implementation-defined cap on a single `raw_bytes` request, so a hostile
/// `len` cannot force an unbounded host allocation (spec 0054 P4 leaves an
/// over-large or negative `len` implementation-defined).
const BYTES_CAP: i32 = 16 * 1024 * 1024;

/// `random.raw_int`: a uniformly-distributed `Int` (i32) drawn from OS entropy
/// (spec 0054 P3).
pub(crate) fn raw_int(_caller: &mut Caller<'_, Host>) -> std::result::Result<i32, wasmi::Error> {
    let mut buf = [0u8; 4];
    getrandom::getrandom(&mut buf).map_err(|err| host_fail(format!("getrandom failed: {err}")))?;
    Ok(i32::from_le_bytes(buf))
}

/// `random.raw_bytes`: `len` cryptographically-secure random bytes as a `Bytes`
/// value allocated in guest memory (spec 0054 P4). A negative `len` yields an
/// empty `Bytes`; an over-large `len` is clamped to `BYTES_CAP`.
pub(crate) fn raw_bytes(
    caller: &mut Caller<'_, Host>,
    len: i32,
) -> std::result::Result<i32, wasmi::Error> {
    let n = len.clamp(0, BYTES_CAP) as usize;
    let mut bytes = vec![0u8; n];
    getrandom::getrandom(&mut bytes)
        .map_err(|err| host_fail(format!("getrandom failed: {err}")))?;
    let memory = memory(caller)?;
    let alloc = alloc_func(caller)?;
    alloc_string(&memory, &alloc, caller, &bytes)
}
