//! In-process execution of the `wasm-wasi` backend output via the pure-Rust
//! [`wasmi`] interpreter, backing `emela run`.
//!
//! The generated module (see `emela-backend-wasm`) imports only two WASI
//! functions — `proc_exit` (always) and `fd_write` (when the program does I/O)
//! — so this shim implements exactly those, rather than pulling in a full WASI
//! implementation. Keeping the host surface this small mirrors spec 0013/0025:
//! the runner supplies precisely the platform functions the module requires.
//!
//! `_start` always ends by calling `proc_exit` (spec's `emit_start`), so a run
//! never returns normally from `_start`; it unwinds through the [`Exit`] host
//! error, which carries the exit code back out.

use std::io::Write;

use wasmi::errors::HostError;
use wasmi::{Caller, Engine, Extern, Linker, Memory, Module, Store};

use crate::error::{Error, Result};
use crate::socket_host::SocketTable;

/// The wasmi store data shared by both run paths: the host-side state the
/// platform functions need. `captured` is `Some` for `emela test` (spec 0040),
/// where stdout/stderr are buffered instead of written to the process streams;
/// `sockets` holds the live `Socket` listeners and connections (spec 0050),
/// which back the `HttpServer` handler (spec 0046) as well as raw sockets.
#[derive(Default)]
pub(crate) struct Host {
    captured: Option<Captured>,
    sockets: SocketTable,
}

impl Host {
    pub(crate) fn sockets_mut(&mut self) -> &mut SocketTable {
        &mut self.sockets
    }

    pub(crate) fn sockets_mut(&mut self) -> &mut SocketTable {
        &mut self.sockets
    }
}

/// WASI `errno` for a bad file descriptor; returned when a program writes to a
/// descriptor other than stdout (1) or stderr (2).
const WASI_EBADF: i32 = 8;

/// Carries `proc_exit`'s code out of the trap that terminates `_start`.
#[derive(Debug)]
struct Exit(i32);

impl std::fmt::Display for Exit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "process exited with code {}", self.0)
    }
}

impl HostError for Exit {}

/// A host-side failure while servicing a WASI call (e.g. an out-of-bounds memory
/// access from a malformed module). Surfaces as a wasm trap.
#[derive(Debug)]
pub(crate) struct HostFail(pub(crate) String);

impl std::fmt::Display for HostFail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl HostError for HostFail {}

fn host_fail(message: impl Into<String>) -> wasmi::Error {
    wasmi::Error::host(HostFail(message.into()))
}

/// Runs a `wasm-wasi` module in-process and returns its process exit code.
///
/// `main`'s `Int` result is the exit code; any other result maps to `0` (both
/// are encoded by `_start`).
pub fn execute(wasm: &[u8]) -> Result<i32> {
    // `Engine::default()` uses `Config::default()`, which enables the
    // bulk-memory proposal the backend relies on (`memory.copy`).
    let engine = Engine::default();
    let module = Module::new(&engine, wasm)
        .map_err(|err| Error::new(format!("failed to load wasm module: {err}")))?;
    let mut store = Store::new(&engine, Host::default());
    let mut linker: Linker<Host> = Linker::new(&engine);
    link_wasi(&mut linker)?;
    link_http(&mut linker)?;
    link_socket(&mut linker)?;

    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(trap_error)?;
    let start = instance
        .get_typed_func::<(), ()>(&store, "_start")
        .map_err(|err| Error::new(format!("wasm module has no runnable `_start`: {err}")))?;

    match start.call(&mut store, ()) {
        // `_start` always calls `proc_exit`, so returning cleanly is unexpected;
        // treat it as a successful exit anyway.
        Ok(()) => Ok(0),
        Err(err) => match err.downcast_ref::<Exit>() {
            Some(Exit(code)) => Ok(*code),
            None => Err(trap_error(err)),
        },
    }
}

/// Links `proc_exit` and `fd_write` (the WASI surface) into `linker`. When the
/// store captures output (`emela test`), `fd_write` buffers into it; otherwise
/// it writes to the process streams.
fn link_wasi(linker: &mut Linker<Host>) -> Result<()> {
    linker
        .func_wrap(
            "wasi_snapshot_preview1",
            "proc_exit",
            |_caller: Caller<'_, Host>, code: i32| -> std::result::Result<(), wasmi::Error> {
                Err(wasmi::Error::host(Exit(code)))
            },
        )
        .map_err(|err| Error::new(format!("failed to link `proc_exit`: {err}")))?;

    // `fd_write(fd, iovs, iovs_len, nwritten)`: write the gathered bytes to
    // stdout/stderr and report the count. Signature matches the backend glue.
    linker
        .func_wrap(
            "wasi_snapshot_preview1",
            "fd_write",
            |mut caller: Caller<'_, Host>,
             fd: i32,
             iovs: i32,
             iovs_len: i32,
             nwritten: i32|
             -> std::result::Result<i32, wasmi::Error> {
                fd_write(&mut caller, fd, iovs, iovs_len, nwritten)
            },
        )
        .map_err(|err| Error::new(format!("failed to link `fd_write`: {err}")))?;
    Ok(())
}

/// Services a `fd_write` call: gather the iovec-described bytes and write them
/// to the target descriptor (or the captured buffers, for `emela test`).
fn fd_write(
    caller: &mut Caller<'_, Host>,
    fd: i32,
    iovs: i32,
    iovs_len: i32,
    nwritten: i32,
) -> std::result::Result<i32, wasmi::Error> {
    let (memory, bytes) = gather_iovs(caller, iovs, iovs_len)?;

    // fd 1 = stdout, fd 2 = stderr; the backend never emits any other fd.
    if caller.data().captured.is_some() {
        match fd {
            1 => caller
                .data_mut()
                .captured
                .as_mut()
                .unwrap()
                .stdout
                .extend_from_slice(&bytes),
            2 => caller
                .data_mut()
                .captured
                .as_mut()
                .unwrap()
                .stderr
                .extend_from_slice(&bytes),
            _ => return Ok(WASI_EBADF),
        }
    } else {
        match fd {
            1 => write_out(std::io::stdout(), &bytes)?,
            2 => write_out(std::io::stderr(), &bytes)?,
            _ => return Ok(WASI_EBADF),
        }
    }

    store_nwritten(&memory, caller, nwritten, bytes.len() as u32)?;
    Ok(0)
}

/// Reads the iovec-described bytes of an `fd_write` call into one buffer,
/// returning the module's memory for the follow-up `nwritten` store.
fn gather_iovs<T>(
    caller: &mut Caller<'_, T>,
    iovs: i32,
    iovs_len: i32,
) -> std::result::Result<(Memory, Vec<u8>), wasmi::Error> {
    let memory = match caller.get_export("memory") {
        Some(Extern::Memory(memory)) => memory,
        _ => return Err(host_fail("module does not export `memory`")),
    };

    // Gather every `[ptr: i32][len: i32]` iovec into one buffer. `Memory::read`
    // bounds-checks, so a malformed pointer/length traps rather than panics.
    let mut bytes = Vec::new();
    for index in 0..iovs_len as usize {
        let entry = iovs as usize + index * 8;
        let mut header = [0u8; 8];
        read_mem(&memory, caller, entry, &mut header)?;
        let ptr = u32::from_le_bytes(header[0..4].try_into().unwrap()) as usize;
        let len = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        let mut chunk = vec![0u8; len];
        read_mem(&memory, caller, ptr, &mut chunk)?;
        bytes.extend_from_slice(&chunk);
    }
    Ok((memory, bytes))
}

fn store_nwritten<T>(
    memory: &Memory,
    caller: &mut Caller<'_, T>,
    nwritten: i32,
    written: u32,
) -> std::result::Result<(), wasmi::Error> {
    memory
        .write(&mut *caller, nwritten as usize, &written.to_le_bytes())
        .map_err(|err| host_fail(format!("failed to store nwritten: {err}")))
}

fn read_mem<T>(
    memory: &Memory,
    caller: &Caller<'_, T>,
    offset: usize,
    buffer: &mut [u8],
) -> std::result::Result<(), wasmi::Error> {
    memory
        .read(caller, offset, buffer)
        .map_err(|err| host_fail(format!("out-of-bounds wasm memory access: {err}")))
}

/// The stdout/stderr a captured execution produced (spec 0040 C6/C9).
#[derive(Default)]
pub(crate) struct Captured {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

/// How a captured execution ended: a clean `proc_exit` or a runtime trap
/// (`panic`/`unreachable`, spec 0040 C4).
pub(crate) enum RunOutcome {
    Exit(i32),
    Trap(String),
}

/// Runs a `wasm-wasi` module in-process like [`execute`], but captures its
/// stdout/stderr into buffers instead of the process streams, and reports a
/// trap as an outcome instead of an error. Backs `emela test` (spec 0040 C5:
/// each test runs in a fresh instance; C6: failure details come from the
/// captured stderr).
pub(crate) fn execute_captured(wasm: &[u8]) -> Result<(RunOutcome, Captured)> {
    let engine = Engine::default();
    let module = Module::new(&engine, wasm)
        .map_err(|err| Error::new(format!("failed to load wasm module: {err}")))?;
    let mut store = Store::new(
        &engine,
        Host {
            captured: Some(Captured::default()),
            ..Default::default()
        },
    );
    let mut linker: Linker<Host> = Linker::new(&engine);
    link_wasi(&mut linker)?;
    link_http(&mut linker)?;
    link_socket(&mut linker)?;

    let instance = match linker.instantiate_and_start(&mut store, &module) {
        Ok(instance) => instance,
        Err(err) => {
            return Ok((
                RunOutcome::Trap(format!("{err}")),
                store.into_data().captured.unwrap_or_default(),
            ));
        }
    };
    let start = instance
        .get_typed_func::<(), ()>(&store, "_start")
        .map_err(|err| Error::new(format!("wasm module has no runnable `_start`: {err}")))?;

    let outcome = match start.call(&mut store, ()) {
        Ok(()) => RunOutcome::Exit(0),
        Err(err) => match err.downcast_ref::<Exit>() {
            Some(Exit(code)) => RunOutcome::Exit(*code),
            None => RunOutcome::Trap(format!("{err}")),
        },
    };
    Ok((outcome, store.into_data().captured.unwrap_or_default()))
}

fn write_out(mut sink: impl Write, bytes: &[u8]) -> std::result::Result<(), wasmi::Error> {
    // Flush eagerly: `proc_exit` ends the process without unwinding Rust's
    // buffered stdout, so unflushed output would be lost.
    sink.write_all(bytes)
        .and_then(|()| sink.flush())
        .map_err(|err| host_fail(format!("failed to write program output: {err}")))
}

/// Links the `Http` client (specs 0043/0044) host function. The server
/// (`HttpServer`, spec 0046) is now a derived effect over `Socket` (spec 0050)
/// implemented in Emela, so it links through `link_socket`, not here. Every
/// import is defined for every run; a module that does not use `Http` never
/// imports it.
fn link_http(linker: &mut Linker<Host>) -> Result<()> {
    linker
        .func_wrap(
            "emela_http",
            "request",
            |mut caller: Caller<'_, Host>, req: i32| -> std::result::Result<i32, wasmi::Error> {
                crate::http_host::request(&mut caller, req)
            },
        )
        .map_err(|err| Error::new(format!("failed to link `emela_http.request`: {err}")))?;
    Ok(())
}

/// Links the `Socket` capability's host functions (spec 0050) into `linker`,
/// backed by `std::net`. Every import is defined for every run; a module that
/// does not use `Socket` simply never imports them.
fn link_socket(linker: &mut Linker<Host>) -> Result<()> {
    linker
        .func_wrap(
            "emela_socket",
            "raw_listen",
            |mut caller: Caller<'_, Host>, port: i32| -> std::result::Result<i32, wasmi::Error> {
                crate::socket_host::raw_listen(&mut caller, port)
            },
        )
        .map_err(|err| Error::new(format!("failed to link `emela_socket.raw_listen`: {err}")))?;
    linker
        .func_wrap(
            "emela_socket",
            "raw_accept",
            |mut caller: Caller<'_, Host>,
             listener: i32|
             -> std::result::Result<i32, wasmi::Error> {
                crate::socket_host::raw_accept(&mut caller, listener)
            },
        )
        .map_err(|err| Error::new(format!("failed to link `emela_socket.raw_accept`: {err}")))?;
    linker
        .func_wrap(
            "emela_socket",
            "raw_read",
            |mut caller: Caller<'_, Host>,
             conn: i32,
             max: i32|
             -> std::result::Result<i32, wasmi::Error> {
                crate::socket_host::raw_read(&mut caller, conn, max)
            },
        )
        .map_err(|err| Error::new(format!("failed to link `emela_socket.raw_read`: {err}")))?;
    linker
        .func_wrap(
            "emela_socket",
            "raw_write",
            |mut caller: Caller<'_, Host>,
             conn: i32,
             data: i32|
             -> std::result::Result<i32, wasmi::Error> {
                crate::socket_host::raw_write(&mut caller, conn, data)
            },
        )
        .map_err(|err| Error::new(format!("failed to link `emela_socket.raw_write`: {err}")))?;
    linker
        .func_wrap(
            "emela_socket",
            "raw_close",
            |mut caller: Caller<'_, Host>, handle: i32| -> std::result::Result<i32, wasmi::Error> {
                crate::socket_host::raw_close(&mut caller, handle)
            },
        )
        .map_err(|err| Error::new(format!("failed to link `emela_socket.raw_close`: {err}")))?;
    Ok(())
}

/// Links the `Socket` capability's host functions (spec 0050) into `linker`,
/// backed by `std::net`. Every import is defined for every run; a module that
/// does not use `Socket` simply never imports them.
fn link_socket(linker: &mut Linker<Host>) -> Result<()> {
    linker
        .func_wrap(
            "emela_socket",
            "raw_listen",
            |mut caller: Caller<'_, Host>, port: i32| -> std::result::Result<i32, wasmi::Error> {
                crate::socket_host::raw_listen(&mut caller, port)
            },
        )
        .map_err(|err| Error::new(format!("failed to link `emela_socket.raw_listen`: {err}")))?;
    linker
        .func_wrap(
            "emela_socket",
            "raw_accept",
            |mut caller: Caller<'_, Host>,
             listener: i32|
             -> std::result::Result<i32, wasmi::Error> {
                crate::socket_host::raw_accept(&mut caller, listener)
            },
        )
        .map_err(|err| Error::new(format!("failed to link `emela_socket.raw_accept`: {err}")))?;
    linker
        .func_wrap(
            "emela_socket",
            "raw_read",
            |mut caller: Caller<'_, Host>,
             conn: i32,
             max: i32|
             -> std::result::Result<i32, wasmi::Error> {
                crate::socket_host::raw_read(&mut caller, conn, max)
            },
        )
        .map_err(|err| Error::new(format!("failed to link `emela_socket.raw_read`: {err}")))?;
    linker
        .func_wrap(
            "emela_socket",
            "raw_write",
            |mut caller: Caller<'_, Host>,
             conn: i32,
             data: i32|
             -> std::result::Result<i32, wasmi::Error> {
                crate::socket_host::raw_write(&mut caller, conn, data)
            },
        )
        .map_err(|err| Error::new(format!("failed to link `emela_socket.raw_write`: {err}")))?;
    linker
        .func_wrap(
            "emela_socket",
            "raw_close",
            |mut caller: Caller<'_, Host>, handle: i32| -> std::result::Result<i32, wasmi::Error> {
                crate::socket_host::raw_close(&mut caller, handle)
            },
        )
        .map_err(|err| Error::new(format!("failed to link `emela_socket.raw_close`: {err}")))?;
    Ok(())
}

/// Renders a wasm trap (panic via `unreachable`, unresolved import, etc.) as a
/// CLI error.
fn trap_error(err: wasmi::Error) -> Error {
    Error::new(format!("wasm runtime error: {err}"))
}
