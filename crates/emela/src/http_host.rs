//! The host side of the `Http` capability (specs 0043/0044) for `emela run`.
//!
//! The wasm backend lowers `http.request` to a call of the imported host
//! function `emela_http.request`, passing a pointer to the guest `Request`
//! record. This module reads that record out of linear memory, performs one
//! synchronous HTTP/1.1 exchange with a small `std::net` client (no extra
//! dependencies), and writes a spec-0011 Result cell back into guest memory
//! (allocated through the module's exported bump allocator `alloc`). Transport
//! failure is reported as an `HttpError` value on the error channel; an HTTP
//! status of any kind is a successful `Response` (spec 0044 H4).
//!
//! The value layouts mirror the wasm backend's ABI: a record is a pointer to
//! consecutive 8-byte field slots; a string is `[len: i32][utf8]`; an
//! `Array<T>` is `[len: i32][elem...]` with 4-byte pointer elements; a
//! no-payload enum is `[tag: i32]`, and a payload variant follows the tag with
//! one 8-byte slot per field.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs};
use std::time::Duration;

use wasmi::{AsContextMut, Caller, Extern, Memory, TypedFunc};

use crate::run::Host;

/// The default per-request timeout (spec 0044 H7). Implementation-defined.
const TIMEOUT: Duration = Duration::from_secs(30);

/// How long a graceful close lingers draining inbound data before giving up
/// (see `graceful_close`). Short so a client that reads its response but leaves
/// the socket open cannot stall the single-threaded accept loop.
const LINGER: Duration = Duration::from_secs(1);

/// `HttpError` variant tags, in declaration order (see `std/http.emel`).
const ERR_INVALID_URL: u32 = 0;
const ERR_CONNECT_FAILED: u32 = 1;
const ERR_TIMEOUT: u32 = 2;
const ERR_TOO_LARGE: u32 = 3;
const ERR_NON_UTF8_BODY: u32 = 4;
const ERR_PROTOCOL: u32 = 5;
const ERR_BIND_FAILED: u32 = 6;
const ERR_CONNECTION_CLOSED: u32 = 7;

/// An implementation-defined cap on a response body (spec 0044: `TooLarge`).
const MAX_BODY: usize = 32 * 1024 * 1024;

/// A `Request` read out of guest memory.
struct HostRequest {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// A transport-level failure, mapped to an `HttpError` variant.
enum ReqError {
    InvalidUrl(String),
    ConnectFailed(String),
    Timeout,
    TooLarge,
    NonUtf8Body,
    Protocol(String),
    BindFailed(String),
    ConnectionClosed,
}

/// A parsed response before it is written back into guest memory.
struct HostResponse {
    status: i32,
    headers: Vec<(String, String)>,
    body: String,
}

/// The live `HttpServer` state (spec 0046): listeners bound by `bind`, keyed by
/// their index, and accepted connections awaiting a `respond`, keyed by a fresh
/// id. Held in the wasmi store data so it persists across host calls.
#[derive(Default)]
pub(crate) struct ServerTable {
    listeners: Vec<Option<TcpListener>>,
    connections: HashMap<u32, TcpStream>,
    next_connection: u32,
}

/// Services one `emela_http.request` call. `req_ptr` is the guest `Request`
/// pointer; the return value is the guest pointer to a spec-0011 Result cell
/// (`[ok][pad][Response | HttpError]`).
pub(crate) fn request(
    caller: &mut Caller<'_, Host>,
    req_ptr: i32,
) -> std::result::Result<i32, wasmi::Error> {
    let memory = memory(caller)?;
    let alloc = alloc_func(caller)?;
    let request = read_request(&memory, caller, req_ptr)?;
    match perform(&request) {
        Ok(response) => {
            let value = write_response(&memory, &alloc, caller, &response)?;
            write_result(&memory, &alloc, caller, true, value)
        }
        Err(err) => {
            let value = write_error(&memory, &alloc, caller, &err)?;
            write_result(&memory, &alloc, caller, false, value)
        }
    }
}

// ---------------------------------------------------------------------------
// The HTTP client
// ---------------------------------------------------------------------------

fn perform(request: &HostRequest) -> std::result::Result<HostResponse, ReqError> {
    let url = parse_url(&request.url)?;
    if url.scheme == "https" {
        // TLS is the host's responsibility (spec 0044 H8); the in-process
        // wasmi runner does not provide it.
        return Err(ReqError::ConnectFailed(
            "https is not supported by the built-in runner".to_string(),
        ));
    }
    let addr = (url.host.as_str(), url.port)
        .to_socket_addrs()
        .map_err(|err| ReqError::ConnectFailed(err.to_string()))?
        .next()
        .ok_or_else(|| ReqError::ConnectFailed(format!("cannot resolve `{}`", url.host)))?;
    let mut stream = TcpStream::connect_timeout(&addr, TIMEOUT).map_err(map_connect_error)?;
    stream
        .set_read_timeout(Some(TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(TIMEOUT)))
        .map_err(|err| ReqError::ConnectFailed(err.to_string()))?;

    let mut head = format!("{} {} HTTP/1.1\r\n", request.method, url.target);
    head.push_str(&format!("host: {}\r\n", url.host_header()));
    head.push_str(&format!("content-length: {}\r\n", request.body.len()));
    head.push_str("connection: close\r\n");
    for (name, value) in &request.headers {
        // The platform supplies host/content-length/connection (spec 0044 H5).
        let lower = name.to_ascii_lowercase();
        if lower == "host" || lower == "content-length" || lower == "connection" {
            continue;
        }
        head.push_str(&format!("{lower}: {value}\r\n"));
    }
    head.push_str("\r\n");

    stream.write_all(head.as_bytes()).map_err(map_io_error)?;
    stream.write_all(&request.body).map_err(map_io_error)?;
    stream.flush().map_err(map_io_error)?;

    let mut raw = Vec::new();
    read_to_limit(&mut stream, &mut raw)?;
    parse_response(&raw)
}

fn read_to_limit(stream: &mut TcpStream, out: &mut Vec<u8>) -> std::result::Result<(), ReqError> {
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                if out.len() + n > MAX_BODY {
                    return Err(ReqError::TooLarge);
                }
                out.extend_from_slice(&buf[..n]);
            }
            Err(err) => return Err(map_io_error(err)),
        }
    }
}

struct Url {
    scheme: String,
    host: String,
    port: u16,
    /// The origin-form request target: path plus optional `?query`.
    target: String,
    default_port: bool,
}

impl Url {
    /// The `Host` header value: host, plus `:port` for a non-default port.
    fn host_header(&self) -> String {
        if self.default_port {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

fn parse_url(url: &str) -> std::result::Result<Url, ReqError> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| ReqError::InvalidUrl(url.to_string()))?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(ReqError::InvalidUrl(url.to_string()));
    }
    let (authority, target) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return Err(ReqError::InvalidUrl(url.to_string()));
    }
    let default_port = if scheme == "https" { 443 } else { 80 };
    let (host, port, is_default) = match authority.rsplit_once(':') {
        Some((host, port)) => {
            let port = port
                .parse::<u16>()
                .map_err(|_| ReqError::InvalidUrl(url.to_string()))?;
            (host.to_string(), port, port == default_port)
        }
        None => (authority.to_string(), default_port, true),
    };
    Ok(Url {
        scheme,
        host,
        port,
        target: target.to_string(),
        default_port: is_default,
    })
}

fn parse_response(raw: &[u8]) -> std::result::Result<HostResponse, ReqError> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| ReqError::Protocol("no header terminator".to_string()))?;
    let header_text = std::str::from_utf8(&raw[..split])
        .map_err(|_| ReqError::Protocol("non-UTF-8 headers".to_string()))?;
    let body_bytes = &raw[split + 4..];

    let mut lines = header_text.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| ReqError::Protocol("empty response".to_string()))?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<i32>().ok())
        .ok_or_else(|| ReqError::Protocol(format!("bad status line: {status_line}")))?;

    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            // Header names are normalized to lowercase (spec 0044 H5).
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }

    let body = String::from_utf8(body_bytes.to_vec()).map_err(|_| ReqError::NonUtf8Body)?;
    Ok(HostResponse {
        status,
        headers,
        body,
    })
}

fn map_connect_error(err: std::io::Error) -> ReqError {
    match err.kind() {
        std::io::ErrorKind::TimedOut => ReqError::Timeout,
        _ => ReqError::ConnectFailed(err.to_string()),
    }
}

fn map_io_error(err: std::io::Error) -> ReqError {
    match err.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => ReqError::Timeout,
        _ => ReqError::Protocol(err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Reading the guest Request
// ---------------------------------------------------------------------------

fn read_request<T>(
    memory: &Memory,
    caller: &mut Caller<'_, T>,
    ptr: i32,
) -> std::result::Result<HostRequest, wasmi::Error> {
    let method_ptr = read_u32(memory, caller, ptr as usize)? as usize;
    let url_ptr = read_u32(memory, caller, ptr as usize + 8)? as usize;
    let headers_ptr = read_u32(memory, caller, ptr as usize + 16)? as usize;
    let body_ptr = read_u32(memory, caller, ptr as usize + 24)? as usize;

    let method = method_name(read_u32(memory, caller, method_ptr)?);
    let url = read_string(memory, caller, url_ptr)?;
    let body = read_string_bytes(memory, caller, body_ptr)?;
    let headers = read_headers(memory, caller, headers_ptr)?;

    Ok(HostRequest {
        method,
        url: String::from_utf8_lossy(&url).into_owned(),
        headers,
        body,
    })
}

fn method_name(tag: u32) -> String {
    match tag {
        0 => "GET",
        1 => "HEAD",
        2 => "POST",
        3 => "PUT",
        4 => "DELETE",
        5 => "PATCH",
        6 => "OPTIONS",
        _ => "GET",
    }
    .to_string()
}

fn read_headers<T>(
    memory: &Memory,
    caller: &mut Caller<'_, T>,
    ptr: usize,
) -> std::result::Result<Vec<(String, String)>, wasmi::Error> {
    let count = read_u32(memory, caller, ptr)? as usize;
    let mut headers = Vec::with_capacity(count);
    for index in 0..count {
        // 4-byte pointer elements follow the length word.
        let header_ptr = read_u32(memory, caller, ptr + 4 + index * 4)? as usize;
        let name = read_string(
            memory,
            caller,
            read_u32(memory, caller, header_ptr)? as usize,
        )?;
        let value = read_string(
            memory,
            caller,
            read_u32(memory, caller, header_ptr + 8)? as usize,
        )?;
        headers.push((
            String::from_utf8_lossy(&name).into_owned(),
            String::from_utf8_lossy(&value).into_owned(),
        ));
    }
    Ok(headers)
}

fn read_string<T>(
    memory: &Memory,
    caller: &mut Caller<'_, T>,
    ptr: usize,
) -> std::result::Result<Vec<u8>, wasmi::Error> {
    read_string_bytes(memory, caller, ptr)
}

fn read_string_bytes<T>(
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

fn read_u32<T>(
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

// ---------------------------------------------------------------------------
// Writing the guest Response / HttpError
// ---------------------------------------------------------------------------

fn write_response<T>(
    memory: &Memory,
    alloc: &TypedFunc<i32, i32>,
    caller: &mut Caller<'_, T>,
    response: &HostResponse,
) -> std::result::Result<i32, wasmi::Error> {
    let headers = write_headers(memory, alloc, caller, &response.headers)?;
    let body = alloc_string(memory, alloc, caller, response.body.as_bytes())?;
    // Response { status: Int, headers: Array<Header>, body: String }.
    let record = guest_alloc(alloc, caller, 24)?;
    write_u32(memory, caller, record as usize, response.status as u32)?;
    write_u32(memory, caller, record as usize + 8, headers as u32)?;
    write_u32(memory, caller, record as usize + 16, body as u32)?;
    Ok(record)
}

fn write_headers<T>(
    memory: &Memory,
    alloc: &TypedFunc<i32, i32>,
    caller: &mut Caller<'_, T>,
    headers: &[(String, String)],
) -> std::result::Result<i32, wasmi::Error> {
    let mut element_ptrs = Vec::with_capacity(headers.len());
    for (name, value) in headers {
        let name_ptr = alloc_string(memory, alloc, caller, name.as_bytes())?;
        let value_ptr = alloc_string(memory, alloc, caller, value.as_bytes())?;
        // Header { name: String, value: String } — two 8-byte slots.
        let record = guest_alloc(alloc, caller, 16)?;
        write_u32(memory, caller, record as usize, name_ptr as u32)?;
        write_u32(memory, caller, record as usize + 8, value_ptr as u32)?;
        element_ptrs.push(record);
    }
    // Array<Header>: [len][ptr...] with 4-byte pointer elements.
    let array = guest_alloc(alloc, caller, 4 + element_ptrs.len() as i32 * 4)?;
    write_u32(memory, caller, array as usize, element_ptrs.len() as u32)?;
    for (index, element) in element_ptrs.iter().enumerate() {
        write_u32(
            memory,
            caller,
            array as usize + 4 + index * 4,
            *element as u32,
        )?;
    }
    Ok(array)
}

fn write_error<T>(
    memory: &Memory,
    alloc: &TypedFunc<i32, i32>,
    caller: &mut Caller<'_, T>,
    err: &ReqError,
) -> std::result::Result<i32, wasmi::Error> {
    match err {
        ReqError::InvalidUrl(msg) => {
            alloc_error_string(memory, alloc, caller, ERR_INVALID_URL, msg)
        }
        ReqError::ConnectFailed(msg) => {
            alloc_error_string(memory, alloc, caller, ERR_CONNECT_FAILED, msg)
        }
        ReqError::Protocol(msg) => alloc_error_string(memory, alloc, caller, ERR_PROTOCOL, msg),
        ReqError::BindFailed(msg) => {
            alloc_error_string(memory, alloc, caller, ERR_BIND_FAILED, msg)
        }
        ReqError::Timeout => alloc_error_tag(memory, alloc, caller, ERR_TIMEOUT),
        ReqError::TooLarge => alloc_error_tag(memory, alloc, caller, ERR_TOO_LARGE),
        ReqError::NonUtf8Body => alloc_error_tag(memory, alloc, caller, ERR_NON_UTF8_BODY),
        ReqError::ConnectionClosed => alloc_error_tag(memory, alloc, caller, ERR_CONNECTION_CLOSED),
    }
}

fn alloc_error_tag<T>(
    memory: &Memory,
    alloc: &TypedFunc<i32, i32>,
    caller: &mut Caller<'_, T>,
    tag: u32,
) -> std::result::Result<i32, wasmi::Error> {
    // A no-payload enum value: [tag].
    let ptr = guest_alloc(alloc, caller, 8)?;
    write_u32(memory, caller, ptr as usize, tag)?;
    Ok(ptr)
}

fn alloc_error_string<T>(
    memory: &Memory,
    alloc: &TypedFunc<i32, i32>,
    caller: &mut Caller<'_, T>,
    tag: u32,
    message: &str,
) -> std::result::Result<i32, wasmi::Error> {
    let string = alloc_string(memory, alloc, caller, message.as_bytes())?;
    // A one-payload variant: [tag][field0].
    let ptr = guest_alloc(alloc, caller, 16)?;
    write_u32(memory, caller, ptr as usize, tag)?;
    write_u32(memory, caller, ptr as usize + 8, string as u32)?;
    Ok(ptr)
}

fn write_result<T>(
    memory: &Memory,
    alloc: &TypedFunc<i32, i32>,
    caller: &mut Caller<'_, T>,
    ok: bool,
    value: i32,
) -> std::result::Result<i32, wasmi::Error> {
    // The spec-0011 Result cell: [ok: i32][pad][value: 8 bytes].
    let cell = guest_alloc(alloc, caller, 16)?;
    write_u32(memory, caller, cell as usize, u32::from(ok))?;
    write_u32(memory, caller, cell as usize + 8, value as u32)?;
    Ok(cell)
}

fn alloc_string<T>(
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

fn write_u32<T>(
    memory: &Memory,
    caller: &mut Caller<'_, T>,
    offset: usize,
    value: u32,
) -> std::result::Result<(), wasmi::Error> {
    memory
        .write(&mut *caller, offset, &value.to_le_bytes())
        .map_err(|err| host_fail(format!("failed to write guest memory: {err}")))
}

fn guest_alloc<T>(
    alloc: &TypedFunc<i32, i32>,
    caller: &mut Caller<'_, T>,
    n: i32,
) -> std::result::Result<i32, wasmi::Error> {
    alloc.call(caller.as_context_mut(), n)
}

fn memory<T>(caller: &mut Caller<'_, T>) -> std::result::Result<Memory, wasmi::Error> {
    match caller.get_export("memory") {
        Some(Extern::Memory(memory)) => Ok(memory),
        _ => Err(host_fail("module does not export `memory`")),
    }
}

fn alloc_func<T>(
    caller: &mut Caller<'_, T>,
) -> std::result::Result<TypedFunc<i32, i32>, wasmi::Error> {
    match caller.get_export("alloc") {
        Some(Extern::Func(func)) => func
            .typed::<i32, i32>(&*caller)
            .map_err(|err| host_fail(format!("`alloc` has an unexpected signature: {err}"))),
        _ => Err(host_fail("module does not export `alloc`")),
    }
}

fn host_fail(message: impl Into<String>) -> wasmi::Error {
    wasmi::Error::host(super::run::HostFail(message.into()))
}

// ---------------------------------------------------------------------------
// The HttpServer capability (spec 0046)
// ---------------------------------------------------------------------------

/// A request parsed from an accepted connection, ready to become a guest
/// `Request` record.
struct ParsedRequest {
    method_tag: u32,
    target: String,
    headers: Vec<(String, String)>,
    body: String,
}

/// Why an incoming request is not delivered to `accept` (spec 0046 S3): the
/// host answers it directly and moves on.
enum IncomingError {
    /// A method outside `Method`; the host answers 501.
    UnknownMethod,
    /// A malformed request or a non-UTF-8 body; the host answers 400.
    BadRequest,
    /// A transport failure reading the request.
    Io(String),
}

/// `HttpServer.bind(port)` (spec 0046 S2): start listening on the loopback
/// interface and return a `Server` handle.
pub(crate) fn server_bind(
    caller: &mut Caller<'_, Host>,
    port: i32,
) -> std::result::Result<i32, wasmi::Error> {
    let memory = memory(caller)?;
    let alloc = alloc_func(caller)?;
    match do_bind(caller.data_mut().servers_mut(), port) {
        Ok(id) => {
            // Server { id: Int } — one field.
            let record = guest_alloc(&alloc, caller, 8)?;
            write_u32(&memory, caller, record as usize, id)?;
            write_result(&memory, &alloc, caller, true, record)
        }
        Err(err) => {
            let value = write_error(&memory, &alloc, caller, &err)?;
            write_result(&memory, &alloc, caller, false, value)
        }
    }
}

/// `HttpServer.accept(server)` (spec 0046 S3): block until a well-formed
/// request arrives and return an `Incoming { id, request }`. Requests the host
/// rejects (unknown method, non-UTF-8 body) are answered directly and skipped.
pub(crate) fn server_accept(
    caller: &mut Caller<'_, Host>,
    server_ptr: i32,
) -> std::result::Result<i32, wasmi::Error> {
    let memory = memory(caller)?;
    let alloc = alloc_func(caller)?;
    let server_id = read_u32(&memory, caller, server_ptr as usize)?;
    match do_accept(caller.data_mut().servers_mut(), server_id) {
        Ok((conn_id, parsed)) => {
            let request = build_guest_request(&memory, &alloc, caller, &parsed)?;
            // Incoming { id: Int, request: Request } — two slots.
            let incoming = guest_alloc(&alloc, caller, 16)?;
            write_u32(&memory, caller, incoming as usize, conn_id)?;
            write_u32(&memory, caller, incoming as usize + 8, request as u32)?;
            write_result(&memory, &alloc, caller, true, incoming)
        }
        Err(err) => {
            let value = write_error(&memory, &alloc, caller, &err)?;
            write_result(&memory, &alloc, caller, false, value)
        }
    }
}

/// `HttpServer.respond(incoming, response)` (spec 0046 S4): send the response
/// on the accepted connection and close it.
pub(crate) fn server_respond(
    caller: &mut Caller<'_, Host>,
    incoming_ptr: i32,
    response_ptr: i32,
) -> std::result::Result<i32, wasmi::Error> {
    let memory = memory(caller)?;
    let alloc = alloc_func(caller)?;
    let conn_id = read_u32(&memory, caller, incoming_ptr as usize)?;
    let response = read_response(&memory, caller, response_ptr)?;
    match do_respond(caller.data_mut().servers_mut(), conn_id, &response) {
        Ok(()) => write_result(&memory, &alloc, caller, true, 0),
        Err(err) => {
            let value = write_error(&memory, &alloc, caller, &err)?;
            write_result(&memory, &alloc, caller, false, value)
        }
    }
}

/// `HttpServer.close(server)` (spec 0046 S5): stop listening.
pub(crate) fn server_close(
    caller: &mut Caller<'_, Host>,
    server_ptr: i32,
) -> std::result::Result<i32, wasmi::Error> {
    let memory = memory(caller)?;
    let alloc = alloc_func(caller)?;
    let server_id = read_u32(&memory, caller, server_ptr as usize)?;
    if let Some(slot) = caller
        .data_mut()
        .servers_mut()
        .listeners
        .get_mut(server_id as usize)
    {
        *slot = None;
    }
    write_result(&memory, &alloc, caller, true, 0)
}

fn do_bind(servers: &mut ServerTable, port: i32) -> std::result::Result<u32, ReqError> {
    let listener = TcpListener::bind(("127.0.0.1", port as u16))
        .map_err(|err| ReqError::BindFailed(err.to_string()))?;
    let id = servers.listeners.len() as u32;
    servers.listeners.push(Some(listener));
    Ok(id)
}

fn do_accept(
    servers: &mut ServerTable,
    server_id: u32,
) -> std::result::Result<(u32, ParsedRequest), ReqError> {
    loop {
        let mut stream = {
            let listener = servers
                .listeners
                .get(server_id as usize)
                .and_then(|slot| slot.as_ref())
                .ok_or(ReqError::ConnectionClosed)?;
            let (stream, _) = listener
                .accept()
                .map_err(|err| ReqError::ConnectFailed(err.to_string()))?;
            stream
        };
        let _ = stream.set_read_timeout(Some(TIMEOUT));
        match parse_incoming(&mut stream) {
            Ok(parsed) => {
                let id = servers.next_connection;
                servers.next_connection += 1;
                servers.connections.insert(id, stream);
                return Ok((id, parsed));
            }
            // The host answers directly and does not deliver (spec 0046 S3).
            Err(IncomingError::UnknownMethod) => {
                let _ = write_status_only(&mut stream, 501);
                graceful_close(stream);
            }
            Err(IncomingError::BadRequest) => {
                let _ = write_status_only(&mut stream, 400);
                graceful_close(stream);
            }
            Err(IncomingError::Io(err)) => return Err(ReqError::Protocol(err)),
        }
        // The rejected connection is closed gracefully; loop for the next.
    }
}

fn do_respond(
    servers: &mut ServerTable,
    conn_id: u32,
    response: &HostResponse,
) -> std::result::Result<(), ReqError> {
    let mut stream = servers
        .connections
        .remove(&conn_id)
        .ok_or(ReqError::ConnectionClosed)?;
    let mut out = format!(
        "HTTP/1.1 {} {}\r\n",
        response.status,
        reason_phrase(response.status)
    );
    for (name, value) in &response.headers {
        let lower = name.to_ascii_lowercase();
        if lower == "content-length" || lower == "connection" {
            continue;
        }
        out.push_str(&format!("{lower}: {value}\r\n"));
    }
    out.push_str(&format!("content-length: {}\r\n", response.body.len()));
    out.push_str("connection: close\r\n\r\n");
    stream
        .write_all(out.as_bytes())
        .and_then(|()| stream.write_all(response.body.as_bytes()))
        .and_then(|()| stream.flush())
        .map_err(|_| ReqError::ConnectionClosed)?;
    graceful_close(stream);
    Ok(())
}

fn parse_incoming(stream: &mut TcpStream) -> std::result::Result<ParsedRequest, IncomingError> {
    let mut received = Vec::new();
    let mut buf = [0u8; 8192];
    // Read the head, then the declared body.
    loop {
        if let Some(head_end) = received.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&received[..head_end]).to_ascii_lowercase();
            let content_length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if content_length > MAX_BODY {
                return Err(IncomingError::BadRequest);
            }
            if received.len() >= head_end + 4 + content_length {
                return finish_incoming(&received, head_end, content_length);
            }
        }
        let n = stream
            .read(&mut buf)
            .map_err(|err| IncomingError::Io(err.to_string()))?;
        if n == 0 {
            // Peer closed before a full request; treat as malformed.
            return Err(IncomingError::BadRequest);
        }
        if received.len() + n > MAX_BODY {
            return Err(IncomingError::BadRequest);
        }
        received.extend_from_slice(&buf[..n]);
    }
}

fn finish_incoming(
    received: &[u8],
    head_end: usize,
    content_length: usize,
) -> std::result::Result<ParsedRequest, IncomingError> {
    let head = std::str::from_utf8(&received[..head_end]).map_err(|_| IncomingError::BadRequest)?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next().ok_or(IncomingError::BadRequest)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or(IncomingError::BadRequest)?;
    let target = parts.next().ok_or(IncomingError::BadRequest)?;
    let method_tag = method_tag(method).ok_or(IncomingError::UnknownMethod)?;

    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }

    let body_bytes = &received[head_end + 4..head_end + 4 + content_length];
    let body = String::from_utf8(body_bytes.to_vec()).map_err(|_| IncomingError::BadRequest)?;

    Ok(ParsedRequest {
        method_tag,
        target: target.to_string(),
        headers,
        body,
    })
}

fn method_tag(method: &str) -> Option<u32> {
    Some(match method {
        "GET" => 0,
        "HEAD" => 1,
        "POST" => 2,
        "PUT" => 3,
        "DELETE" => 4,
        "PATCH" => 5,
        "OPTIONS" => 6,
        _ => return None,
    })
}

/// Closes `stream` gracefully so the client sees a FIN, never an RST.
///
/// The connection uses `connection: close`, so the host tears it down after
/// each response (spec 0046 S4: the teardown is the host's responsibility and
/// unobservable). A plain `close` — dropping the `TcpStream` — is abortive on
/// macOS/BSD when the socket's receive buffer still holds unread data: the
/// kernel emits an RST, and that RST can discard a response the client has
/// buffered but not yet read, surfacing as `ConnectionReset` on the client.
///
/// Instead, half-close the write side first (`shutdown(Write)` sends the FIN
/// that lets the client read the whole response and reach EOF), then drain
/// inbound data until the client closes too or a short linger elapses. The
/// final drop then finds an empty receive buffer and closes cleanly.
fn graceful_close(mut stream: TcpStream) {
    let _ = stream.shutdown(Shutdown::Write);
    let _ = stream.set_read_timeout(Some(LINGER));
    let mut sink = [0u8; 8192];
    let mut drained = 0usize;
    while let Ok(n) = stream.read(&mut sink) {
        // `Ok(0)` is the peer's own close; stop before a timeout or error.
        if n == 0 {
            break;
        }
        drained += n;
        if drained > MAX_BODY {
            break;
        }
    }
    // `stream` drops here, closing the (now fully drained) connection.
}

fn write_status_only(stream: &mut TcpStream, status: u16) -> std::io::Result<()> {
    let out = format!(
        "HTTP/1.1 {status} {}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
        reason_phrase(status as i32)
    );
    stream.write_all(out.as_bytes())?;
    stream.flush()
}

fn reason_phrase(status: i32) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        _ => "Status",
    }
}

/// Builds a guest `Request` record from a parsed incoming request.
fn build_guest_request(
    memory: &Memory,
    alloc: &TypedFunc<i32, i32>,
    caller: &mut Caller<'_, Host>,
    parsed: &ParsedRequest,
) -> std::result::Result<i32, wasmi::Error> {
    // Method is a no-payload enum value: [tag].
    let method = guest_alloc(alloc, caller, 8)?;
    write_u32(memory, caller, method as usize, parsed.method_tag)?;
    let url = alloc_string(memory, alloc, caller, parsed.target.as_bytes())?;
    let headers = write_headers(memory, alloc, caller, &parsed.headers)?;
    let body = alloc_string(memory, alloc, caller, parsed.body.as_bytes())?;
    // Request { method, url, headers, body } — four slots.
    let record = guest_alloc(alloc, caller, 32)?;
    write_u32(memory, caller, record as usize, method as u32)?;
    write_u32(memory, caller, record as usize + 8, url as u32)?;
    write_u32(memory, caller, record as usize + 16, headers as u32)?;
    write_u32(memory, caller, record as usize + 24, body as u32)?;
    Ok(record)
}

/// Reads a guest `Response` record into host data (for `respond`).
fn read_response(
    memory: &Memory,
    caller: &mut Caller<'_, Host>,
    ptr: i32,
) -> std::result::Result<HostResponse, wasmi::Error> {
    let status = read_u32(memory, caller, ptr as usize)? as i32;
    let headers_ptr = read_u32(memory, caller, ptr as usize + 8)? as usize;
    let body_ptr = read_u32(memory, caller, ptr as usize + 16)? as usize;
    let headers = read_headers(memory, caller, headers_ptr)?;
    let body = read_string_bytes(memory, caller, body_ptr)?;
    Ok(HostResponse {
        status,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}
