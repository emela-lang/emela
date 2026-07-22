//! Shared guest-memory ABI helpers for the `emela run` wasmi hosts.
//!
//! Every host-provided platform function (the `Http` client, `HttpServer`, and
//! `Socket`) reads its arguments out of, and allocates its structured results
//! into, the guest's linear memory through the module's exported bump allocator
//! `alloc`. These helpers encode the wasm backend's value ABI once so each host
//! module can share it:
//!
//! - a string / `Bytes` is `[len: i32][bytes]`;
//! - a record is a pointer to consecutive 8-byte field slots;
//! - a no-payload enum is `[tag: i32]`, a payload variant `[tag][field...]`;
//! - a fallible result is a spec-0011 cell `[ok: i32][pad][value: 8 bytes]`.

use wasmi::{AsContextMut, Caller, Extern, Memory, TypedFunc};

/// A host-side failure while servicing a call (e.g. an out-of-bounds memory
/// access from a malformed module). Surfaces as a wasm trap.
pub(crate) fn host_fail(message: impl Into<String>) -> wasmi::Error {
    wasmi::Error::host(super::run::HostFail(message.into()))
}

/// The module's exported linear memory.
pub(crate) fn memory<T>(caller: &mut Caller<'_, T>) -> std::result::Result<Memory, wasmi::Error> {
    match caller.get_export("memory") {
        Some(Extern::Memory(memory)) => Ok(memory),
        _ => Err(host_fail("module does not export `memory`")),
    }
}

/// The module's exported bump allocator (`alloc: (i32) -> i32`).
pub(crate) fn alloc_func<T>(
    caller: &mut Caller<'_, T>,
) -> std::result::Result<TypedFunc<i32, i32>, wasmi::Error> {
    match caller.get_export("alloc") {
        Some(Extern::Func(func)) => func
            .typed::<i32, i32>(&*caller)
            .map_err(|err| host_fail(format!("`alloc` has an unexpected signature: {err}"))),
        _ => Err(host_fail("module does not export `alloc`")),
    }
}

/// Reads a little-endian `u32` at `offset`.
pub(crate) fn read_u32<T>(
    memory: &Memory,
    caller: &Caller<'_, T>,
    offset: usize,
) -> std::result::Result<u32, wasmi::Error> {
    let mut buf = [0u8; 4];
    memory
        .read(caller, offset, &mut buf)
        .map_err(|err| host_fail(format!("out-of-bounds wasm memory access: {err}")))?;
    Ok(u32::from_le_bytes(buf))
}

/// Reads a `[len: i32][bytes]` value (a string or `Bytes`) into a byte vector.
pub(crate) fn read_string_bytes<T>(
    memory: &Memory,
    caller: &mut Caller<'_, T>,
    ptr: usize,
) -> std::result::Result<Vec<u8>, wasmi::Error> {
    let len = read_u32(memory, caller, ptr)? as usize;
    let mut bytes = vec![0u8; len];
    memory
        .read(&*caller, ptr + 4, &mut bytes)
        .map_err(|err| host_fail(format!("out-of-bounds string read: {err}")))?;
    Ok(bytes)
}

/// Reads a `[len: i32][utf8]` string value into a byte vector (an alias of
/// [`read_string_bytes`] kept for call-site readability).
pub(crate) fn read_string<T>(
    memory: &Memory,
    caller: &mut Caller<'_, T>,
    ptr: usize,
) -> std::result::Result<Vec<u8>, wasmi::Error> {
    read_string_bytes(memory, caller, ptr)
}

/// Writes a little-endian `u32` at `offset`.
pub(crate) fn write_u32<T>(
    memory: &Memory,
    caller: &mut Caller<'_, T>,
    offset: usize,
    value: u32,
) -> std::result::Result<(), wasmi::Error> {
    memory
        .write(&mut *caller, offset, &value.to_le_bytes())
        .map_err(|err| host_fail(format!("failed to write guest memory: {err}")))
}

/// Allocates a `[len: i32][bytes]` value (a string or `Bytes`) in guest memory.
pub(crate) fn alloc_string<T>(
    memory: &Memory,
    alloc: &TypedFunc<i32, i32>,
    caller: &mut Caller<'_, T>,
    bytes: &[u8],
) -> std::result::Result<i32, wasmi::Error> {
    let ptr = guest_alloc(alloc, caller, 4 + bytes.len() as i32)?;
    write_u32(memory, caller, ptr as usize, bytes.len() as u32)?;
    memory
        .write(&mut *caller, ptr as usize + 4, bytes)
        .map_err(|err| host_fail(format!("failed to write string into guest memory: {err}")))?;
    Ok(ptr)
}

/// Allocates `n` bytes in guest memory through the exported bump allocator.
pub(crate) fn guest_alloc<T>(
    alloc: &TypedFunc<i32, i32>,
    caller: &mut Caller<'_, T>,
    n: i32,
) -> std::result::Result<i32, wasmi::Error> {
    alloc.call(caller.as_context_mut(), n)
}

/// Allocates a no-payload enum value `[tag: i32]` in guest memory.
pub(crate) fn alloc_enum_tag<T>(
    memory: &Memory,
    alloc: &TypedFunc<i32, i32>,
    caller: &mut Caller<'_, T>,
    tag: u32,
) -> std::result::Result<i32, wasmi::Error> {
    let ptr = guest_alloc(alloc, caller, 8)?;
    write_u32(memory, caller, ptr as usize, tag)?;
    Ok(ptr)
}

/// Allocates a single-`String`-payload enum variant `[tag: i32][pad][str_ptr]`
/// in guest memory (the shape shared by `HttpError`/`SocketError` string
/// variants).
pub(crate) fn alloc_enum_string<T>(
    memory: &Memory,
    alloc: &TypedFunc<i32, i32>,
    caller: &mut Caller<'_, T>,
    tag: u32,
    message: &str,
) -> std::result::Result<i32, wasmi::Error> {
    let string = alloc_string(memory, alloc, caller, message.as_bytes())?;
    let ptr = guest_alloc(alloc, caller, 16)?;
    write_u32(memory, caller, ptr as usize, tag)?;
    write_u32(memory, caller, ptr as usize + 8, string as u32)?;
    Ok(ptr)
}

/// Writes a spec-0011 Result cell `[ok: i32][pad][value: 8 bytes]` and returns
/// its guest pointer. `value` is the guest pointer to the ok/err payload.
pub(crate) fn write_result<T>(
    memory: &Memory,
    alloc: &TypedFunc<i32, i32>,
    caller: &mut Caller<'_, T>,
    ok: bool,
    value: i32,
) -> std::result::Result<i32, wasmi::Error> {
    let cell = guest_alloc(alloc, caller, 16)?;
    write_u32(memory, caller, cell as usize, u32::from(ok))?;
    write_u32(memory, caller, cell as usize + 8, value as u32)?;
    Ok(cell)
}
