//! The host side of the `Fs` capability (spec 0055) for `emela run`.
//!
//! The wasm backend lowers each `fs.*` operation to a call of an imported host
//! function in the `emela_fs` module. This module implements those with blocking
//! `std::fs` operations: a table of open file handles keyed by a host-issued
//! id, reading/writing through the shared ABI (`host_abi`). Host failure is
//! reported as an `FsError` value on the spec-0043 error channel.
//!
//! `emela run` is a development runner; the standard `wasi:filesystem` output
//! that runs under `wasmtime` is the component backend's job (spec 0055
//! Compilation Notes).

use std::collections::HashMap;
use std::io::Read;
use std::io::Write;

use wasmi::Caller;

use crate::host_abi::{
    alloc_enum_string, alloc_func, alloc_string, guest_alloc, memory, read_string, read_u32,
    write_result, write_u32,
};
use crate::run::Host;

/// `FsError` variant tags, in declaration order (see `std/fs.emel`).
const ERR_NOT_FOUND: u32 = 0;
const ERR_PERMISSION_DENIED: u32 = 1;
const ERR_IO: u32 = 2;

/// An implementation-defined cap on a single `read`, matching `Socket`'s cap.
const READ_CAP: i32 = 16 * 1024 * 1024;

/// A host-side file-system failure, mapped to an `FsError` variant.
enum FsError {
    NotFound(String),
    PermissionDenied(String),
    Io(String),
}

impl FsError {
    fn from_io(err: &std::io::Error, path: &str) -> Self {
        match err.kind() {
            std::io::ErrorKind::NotFound => FsError::NotFound(path.to_string()),
            std::io::ErrorKind::PermissionDenied => FsError::PermissionDenied(err.to_string()),
            _ => FsError::Io(err.to_string()),
        }
    }

    fn io(msg: impl Into<String>) -> Self {
        FsError::Io(msg.into())
    }
}

/// The live `Fs` state (spec 0055): a table of open file handles sharing one id
/// space. Held in the wasmi store data so it persists across host calls.
#[derive(Default)]
pub(crate) struct FileTable {
    handles: HashMap<u32, std::fs::File>,
    next_id: u32,
}

impl FileTable {
    fn insert(&mut self, file: std::fs::File) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.handles.insert(id, file);
        id
    }
}

/// `Fs.open_read(path)` (spec 0055): open a file for reading (O_RDONLY),
/// returning a `File { id }` on the result channel.
pub(crate) fn open_read(
    caller: &mut Caller<'_, Host>,
    path_ptr: i32,
) -> std::result::Result<i32, wasmi::Error> {
    let memory = memory(caller)?;
    let alloc = alloc_func(caller)?;
    let path_bytes = read_string(&memory, caller, path_ptr as usize)?;
    let path = String::from_utf8_lossy(&path_bytes);
    match std::fs::File::open(path.as_ref()) {
        Ok(file) => {
            let id = caller.data_mut().files_mut().insert(file);
            let record = file_record(&memory, &alloc, caller, id)?;
            write_result(&memory, &alloc, caller, true, record)
        }
        Err(err) => {
            let value = write_fs_error(&memory, &alloc, caller, &FsError::from_io(&err, &path))?;
            write_result(&memory, &alloc, caller, false, value)
        }
    }
}

/// `Fs.open_write(path)` (spec 0055): open a file for writing (O_WRONLY |
/// O_CREAT | O_TRUNC), returning a `File { id }` on the result channel.
pub(crate) fn open_write(
    caller: &mut Caller<'_, Host>,
    path_ptr: i32,
) -> std::result::Result<i32, wasmi::Error> {
    let memory = memory(caller)?;
    let alloc = alloc_func(caller)?;
    let path_bytes = read_string(&memory, caller, path_ptr as usize)?;
    let path = String::from_utf8_lossy(&path_bytes);
    match std::fs::File::create(path.as_ref()) {
        Ok(file) => {
            let id = caller.data_mut().files_mut().insert(file);
            let record = file_record(&memory, &alloc, caller, id)?;
            write_result(&memory, &alloc, caller, true, record)
        }
        Err(err) => {
            let value = write_fs_error(&memory, &alloc, caller, &FsError::from_io(&err, &path))?;
            write_result(&memory, &alloc, caller, false, value)
        }
    }
}

/// `Fs.read(file, max)` (spec 0055): read up to `max` bytes; a zero-length
/// result is EOF.
pub(crate) fn read(
    caller: &mut Caller<'_, Host>,
    file_ptr: i32,
    max: i32,
) -> std::result::Result<i32, wasmi::Error> {
    let memory = memory(caller)?;
    let alloc = alloc_func(caller)?;
    let file_id = read_u32(&memory, caller, file_ptr as usize)?;
    let table = caller.data_mut().files_mut();
    match table.handles.get_mut(&file_id) {
        Some(file) => {
            let cap = max.clamp(0, READ_CAP) as usize;
            let mut buf = vec![0u8; cap];
            match file.read(&mut buf) {
                Ok(n) => {
                    buf.truncate(n);
                    let value = alloc_string(&memory, &alloc, caller, &buf)?;
                    write_result(&memory, &alloc, caller, true, value)
                }
                Err(err) => {
                    let value =
                        write_fs_error(&memory, &alloc, caller, &FsError::io(err.to_string()))?;
                    write_result(&memory, &alloc, caller, false, value)
                }
            }
        }
        None => {
            let value = write_fs_error(&memory, &alloc, caller, &FsError::io("bad file handle"))?;
            write_result(&memory, &alloc, caller, false, value)
        }
    }
}

/// `Fs.write(file, data)` (spec 0055): write `data` in full.
pub(crate) fn write(
    caller: &mut Caller<'_, Host>,
    file_ptr: i32,
    data_ptr: i32,
) -> std::result::Result<i32, wasmi::Error> {
    let memory = memory(caller)?;
    let alloc = alloc_func(caller)?;
    let file_id = read_u32(&memory, caller, file_ptr as usize)?;
    let data = read_string(&memory, caller, data_ptr as usize)?;
    match do_write(caller.data_mut().files_mut(), file_id, &data) {
        Ok(()) => write_result(&memory, &alloc, caller, true, 0),
        Err(err) => {
            let value = write_fs_error(&memory, &alloc, caller, &err)?;
            write_result(&memory, &alloc, caller, false, value)
        }
    }
}

/// `Fs.close(handle)` (spec 0055): release a File by its id. Infallible; a
/// double close (or an unknown id) is harmless. Returns Unit (0).
pub(crate) fn close(
    caller: &mut Caller<'_, Host>,
    handle: i32,
) -> std::result::Result<i32, wasmi::Error> {
    let _ = caller
        .data_mut()
        .files_mut()
        .handles
        .remove(&(handle as u32));
    Ok(0)
}

fn do_write(table: &mut FileTable, file_id: u32, data: &[u8]) -> std::result::Result<(), FsError> {
    let file = table
        .handles
        .get_mut(&file_id)
        .ok_or_else(|| FsError::io("bad file handle"))?;
    file.write_all(data)
        .and_then(|()| file.flush())
        .map_err(|err| FsError::io(err.to_string()))
}

/// Allocates a `File` record `{ id: Int }` (one 8-byte field slot) in guest
/// memory.
fn file_record(
    memory: &wasmi::Memory,
    alloc: &wasmi::TypedFunc<i32, i32>,
    caller: &mut Caller<'_, Host>,
    id: u32,
) -> std::result::Result<i32, wasmi::Error> {
    let record = guest_alloc(alloc, caller, 8)?;
    write_u32(memory, caller, record as usize, id)?;
    Ok(record)
}

fn write_fs_error(
    memory: &wasmi::Memory,
    alloc: &wasmi::TypedFunc<i32, i32>,
    caller: &mut Caller<'_, Host>,
    err: &FsError,
) -> std::result::Result<i32, wasmi::Error> {
    match err {
        FsError::NotFound(msg) => alloc_enum_string(memory, alloc, caller, ERR_NOT_FOUND, msg),
        FsError::PermissionDenied(msg) => {
            alloc_enum_string(memory, alloc, caller, ERR_PERMISSION_DENIED, msg)
        }
        FsError::Io(msg) => alloc_enum_string(memory, alloc, caller, ERR_IO, msg),
    }
}
