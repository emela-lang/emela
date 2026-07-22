//! The host side of the `Socket` capability (spec 0050) for `emela run`.
//!
//! The wasm backend lowers each `socket.raw_*` operation to a call of an
//! imported host function in the `emela_socket` module. This module implements
//! those with a small blocking `std::net` server (no extra dependencies): a
//! unified table of listeners and connections keyed by a host-issued id, read
//! out of / written into the guest's linear memory through the shared ABI
//! (`host_abi`). Transport failure is reported as a `SocketError` value on the
//! spec-0043 error channel.
//!
//! `emela run` is a development runner; the standard `wasi:sockets` output that
//! runs under `wasmtime` is the component backend's job (spec 0050 Compilation
//! Notes). Listeners bind the loopback interface here, matching the HTTP runner.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use wasmi::Caller;

use crate::host_abi::{
    alloc_enum_string, alloc_enum_tag, alloc_func, alloc_string, guest_alloc, memory,
    read_string_bytes, read_u32, write_result, write_u32,
};
use crate::run::Host;

/// `SocketError` variant tags, in declaration order (see `std/socket.emel`).
const ERR_BIND_FAILED: u32 = 0;
const ERR_ACCEPT_FAILED: u32 = 1;
const ERR_CONNECTION_CLOSED: u32 = 2;
const ERR_IO: u32 = 3;

/// An implementation-defined cap on a single `read`, so a hostile `max` cannot
/// force an unbounded host allocation. `read` may return fewer than `max` bytes
/// (spec 0050 P5: "up to `max`"), so clamping the buffer is conformant.
const READ_CAP: i32 = 16 * 1024 * 1024;

/// A host-side transport failure, mapped to a `SocketError` variant.
enum SockError {
    BindFailed(String),
    AcceptFailed(String),
    ConnectionClosed,
    Io(String),
}

/// A live socket handle: either a bound listener or an accepted connection.
enum Handle {
    Listener(TcpListener),
    Connection(TcpStream),
}

/// The live `Socket` state (spec 0050): listeners and connections share one id
/// space so `close(handle)` can take either a `Listener.id` or a
/// `Connection.id` (spec 0050 P7). Held in the wasmi store data so it persists
/// across host calls.
#[derive(Default)]
pub(crate) struct SocketTable {
    handles: HashMap<u32, Handle>,
    next_id: u32,
}

impl SocketTable {
    /// Registers `handle` under a fresh id and returns it.
    fn insert(&mut self, handle: Handle) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.handles.insert(id, handle);
        id
    }
}

/// `Socket.listen(port)` (spec 0050 P3): bind + listen on the loopback
/// interface, returning a `Listener { id }`.
pub(crate) fn raw_listen(
    caller: &mut Caller<'_, Host>,
    port: i32,
) -> std::result::Result<i32, wasmi::Error> {
    let memory = memory(caller)?;
    let alloc = alloc_func(caller)?;
    match do_listen(caller.data_mut().sockets_mut(), port) {
        Ok(id) => {
            let record = handle_record(&memory, &alloc, caller, id)?;
            write_result(&memory, &alloc, caller, true, record)
        }
        Err(err) => {
            let value = write_socket_error(&memory, &alloc, caller, &err)?;
            write_result(&memory, &alloc, caller, false, value)
        }
    }
}

/// `Socket.accept(listener)` (spec 0050 P4): block until a connection arrives
/// and return a `Connection { id }`.
pub(crate) fn raw_accept(
    caller: &mut Caller<'_, Host>,
    listener_ptr: i32,
) -> std::result::Result<i32, wasmi::Error> {
    let memory = memory(caller)?;
    let alloc = alloc_func(caller)?;
    let listener_id = read_u32(&memory, caller, listener_ptr as usize)?;
    match do_accept(caller.data_mut().sockets_mut(), listener_id) {
        Ok(id) => {
            let record = handle_record(&memory, &alloc, caller, id)?;
            write_result(&memory, &alloc, caller, true, record)
        }
        Err(err) => {
            let value = write_socket_error(&memory, &alloc, caller, &err)?;
            write_result(&memory, &alloc, caller, false, value)
        }
    }
}

/// `Socket.read(conn, max)` (spec 0050 P5): read up to `max` bytes; a
/// zero-length result is EOF.
pub(crate) fn raw_read(
    caller: &mut Caller<'_, Host>,
    conn_ptr: i32,
    max: i32,
) -> std::result::Result<i32, wasmi::Error> {
    let memory = memory(caller)?;
    let alloc = alloc_func(caller)?;
    let conn_id = read_u32(&memory, caller, conn_ptr as usize)?;
    match do_read(caller.data_mut().sockets_mut(), conn_id, max) {
        Ok(bytes) => {
            let value = alloc_string(&memory, &alloc, caller, &bytes)?;
            write_result(&memory, &alloc, caller, true, value)
        }
        Err(err) => {
            let value = write_socket_error(&memory, &alloc, caller, &err)?;
            write_result(&memory, &alloc, caller, false, value)
        }
    }
}

/// `Socket.write(conn, data)` (spec 0050 P6): write `data` in full.
pub(crate) fn raw_write(
    caller: &mut Caller<'_, Host>,
    conn_ptr: i32,
    data_ptr: i32,
) -> std::result::Result<i32, wasmi::Error> {
    let memory = memory(caller)?;
    let alloc = alloc_func(caller)?;
    let conn_id = read_u32(&memory, caller, conn_ptr as usize)?;
    let data = read_string_bytes(&memory, caller, data_ptr as usize)?;
    match do_write(caller.data_mut().sockets_mut(), conn_id, &data) {
        // Unit is represented by a 0 payload in the Result cell.
        Ok(()) => write_result(&memory, &alloc, caller, true, 0),
        Err(err) => {
            let value = write_socket_error(&memory, &alloc, caller, &err)?;
            write_result(&memory, &alloc, caller, false, value)
        }
    }
}

/// `Socket.close(handle)` (spec 0050 P7): release a listener or connection by
/// its id. Infallible; a double close (or an unknown id) is harmless. Returns
/// Unit (0).
pub(crate) fn raw_close(
    caller: &mut Caller<'_, Host>,
    handle: i32,
) -> std::result::Result<i32, wasmi::Error> {
    caller
        .data_mut()
        .sockets_mut()
        .handles
        .remove(&(handle as u32));
    Ok(0)
}

fn do_listen(table: &mut SocketTable, port: i32) -> std::result::Result<u32, SockError> {
    let listener = TcpListener::bind(("127.0.0.1", port as u16))
        .map_err(|err| SockError::BindFailed(err.to_string()))?;
    Ok(table.insert(Handle::Listener(listener)))
}

fn do_accept(table: &mut SocketTable, listener_id: u32) -> std::result::Result<u32, SockError> {
    // Borrow the listener only long enough to accept (which blocks); the
    // returned stream is owned, so the borrow ends before we insert it.
    let stream = match table.handles.get(&listener_id) {
        Some(Handle::Listener(listener)) => {
            let (stream, _addr) = listener
                .accept()
                .map_err(|err| SockError::AcceptFailed(err.to_string()))?;
            stream
        }
        // A missing id, a closed listener, or an id that names a connection.
        _ => return Err(SockError::ConnectionClosed),
    };
    Ok(table.insert(Handle::Connection(stream)))
}

fn do_read(
    table: &mut SocketTable,
    conn_id: u32,
    max: i32,
) -> std::result::Result<Vec<u8>, SockError> {
    let stream = match table.handles.get_mut(&conn_id) {
        Some(Handle::Connection(stream)) => stream,
        _ => return Err(SockError::ConnectionClosed),
    };
    let cap = max.clamp(0, READ_CAP) as usize;
    let mut buf = vec![0u8; cap];
    let n = stream
        .read(&mut buf)
        .map_err(|err| SockError::Io(err.to_string()))?;
    buf.truncate(n);
    Ok(buf)
}

fn do_write(
    table: &mut SocketTable,
    conn_id: u32,
    data: &[u8],
) -> std::result::Result<(), SockError> {
    let stream = match table.handles.get_mut(&conn_id) {
        Some(Handle::Connection(stream)) => stream,
        _ => return Err(SockError::ConnectionClosed),
    };
    stream
        .write_all(data)
        .and_then(|()| stream.flush())
        .map_err(|_| SockError::ConnectionClosed)
}

/// Allocates a `Listener`/`Connection` record `{ id: Int }` (one 8-byte field
/// slot) in guest memory.
fn handle_record(
    memory: &wasmi::Memory,
    alloc: &wasmi::TypedFunc<i32, i32>,
    caller: &mut Caller<'_, Host>,
    id: u32,
) -> std::result::Result<i32, wasmi::Error> {
    let record = guest_alloc(alloc, caller, 8)?;
    write_u32(memory, caller, record as usize, id)?;
    Ok(record)
}

fn write_socket_error(
    memory: &wasmi::Memory,
    alloc: &wasmi::TypedFunc<i32, i32>,
    caller: &mut Caller<'_, Host>,
    err: &SockError,
) -> std::result::Result<i32, wasmi::Error> {
    match err {
        SockError::BindFailed(msg) => {
            alloc_enum_string(memory, alloc, caller, ERR_BIND_FAILED, msg)
        }
        SockError::AcceptFailed(msg) => {
            alloc_enum_string(memory, alloc, caller, ERR_ACCEPT_FAILED, msg)
        }
        SockError::Io(msg) => alloc_enum_string(memory, alloc, caller, ERR_IO, msg),
        SockError::ConnectionClosed => alloc_enum_tag(memory, alloc, caller, ERR_CONNECTION_CLOSED),
    }
}
