//! Minimal JSON-RPC 2.0 transport over stdio for the language server (spec
//! 0033): `Content-Length` header framing, request/notification reading, and
//! response/notification writing. Hand-written on serde_json alone, keeping
//! the compiler's zero-dependency surface.

use std::io::{self, BufRead, Write};

use serde::Deserialize;
use serde_json::{Value, json};

/// JSON-RPC: the method does not exist.
pub(crate) const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC: the payload was not a valid request object.
pub(crate) const PARSE_ERROR: i64 = -32700;
/// LSP: a request arrived before `initialize`.
pub(crate) const SERVER_NOT_INITIALIZED: i64 = -32002;

/// An incoming request (`id` + `method`) or notification (`method` only).
/// A message with neither is malformed; one with only `id` would be a response
/// to a server-initiated request, which this server never sends.
#[derive(Debug, Deserialize)]
pub(crate) struct Message {
    pub(crate) id: Option<Value>,
    pub(crate) method: Option<String>,
    #[serde(default)]
    pub(crate) params: Value,
}

/// Reads one framed message. `Ok(None)` means EOF (the client hung up);
/// a body that is not valid JSON comes back as a `Message` with no `method`
/// and no `id`, which the dispatcher answers with [`PARSE_ERROR`].
pub(crate) fn read_message(input: &mut impl BufRead) -> io::Result<Option<Message>> {
    let mut content_length: Option<usize> = None;
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        // `Content-Type` is the only other header the base protocol defines;
        // it is ignored.
        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = value.trim().parse().ok();
        }
    }
    let Some(length) = content_length else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing Content-Length header",
        ));
    };
    let mut body = vec![0u8; length];
    input.read_exact(&mut body)?;
    Ok(Some(serde_json::from_slice(&body).unwrap_or(Message {
        id: None,
        method: None,
        params: Value::Null,
    })))
}

fn write_frame(out: &mut impl Write, payload: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(payload)?;
    write!(out, "Content-Length: {}\r\n\r\n", body.len())?;
    out.write_all(&body)?;
    out.flush()
}

pub(crate) fn write_response(out: &mut impl Write, id: &Value, result: Value) -> io::Result<()> {
    write_frame(out, &json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

pub(crate) fn write_error(
    out: &mut impl Write,
    id: &Value,
    code: i64,
    message: &str,
) -> io::Result<()> {
    write_frame(
        out,
        &json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
    )
}

pub(crate) fn write_notification(
    out: &mut impl Write,
    method: &str,
    params: Value,
) -> io::Result<()> {
    write_frame(
        out,
        &json!({"jsonrpc": "2.0", "method": method, "params": params}),
    )
}
