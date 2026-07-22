//! The platform-function interface (spec 0013).
//!
//! Platform functions are the language-defined set of operations that produce
//! capability effects (spec 0009). The compiler implements none of them; a
//! backend supplies the implementation for the subset it provides. Emela source
//! references them with `extern fn` and may not name a backend, so source stays
//! backend-independent.

use crate::types::Type;

/// One entry of the platform interface: a qualified name, a signature, and the
/// capability effect it produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformFn {
    pub path: Vec<String>,
    pub name: String,
    pub params: Vec<Type>,
    pub ret: Type,
    /// The error type the host may report (spec 0043). A fallible platform
    /// function delivers host failure through the ordinary `throws` channel
    /// (spec 0011); `None` means the entry cannot fail.
    pub throws: Option<Type>,
    pub capability: String,
}

impl PlatformFn {
    /// The qualified name used as the ABI key, e.g. `io.write_stdout`.
    pub fn canonical(&self) -> String {
        let mut out = self.path.join(".");
        if !out.is_empty() {
            out.push('.');
        }
        out.push_str(&self.name);
        out
    }
}

/// The normative platform interface. Entries may declare `throws E`
/// (spec 0043) to report host failure on the error channel; the original
/// `io`/`clock` set stays infallible.
pub fn platform_interface() -> Vec<PlatformFn> {
    vec![
        PlatformFn {
            path: vec!["io".to_string()],
            name: "write_stdout".to_string(),
            params: vec![Type::String],
            ret: Type::Unit,
            throws: None,
            capability: "Io".to_string(),
        },
        PlatformFn {
            path: vec!["io".to_string()],
            name: "write_stderr".to_string(),
            params: vec![Type::String],
            ret: Type::Unit,
            throws: None,
            capability: "Io".to_string(),
        },
        PlatformFn {
            path: vec!["clock".to_string()],
            name: "monotonic_seconds".to_string(),
            params: vec![],
            ret: Type::Int,
            throws: None,
            capability: "Clock".to_string(),
        },
        // One synchronous HTTP exchange (spec 0044); the first fallible entry
        // (spec 0043). `Request`/`Response`/`HttpError` are the named types
        // declared by the embedded `std.http`.
        PlatformFn {
            path: vec!["http".to_string()],
            name: "request".to_string(),
            params: vec![Type::Enum("Request".to_string(), Vec::new())],
            ret: Type::Enum("Response".to_string(), Vec::new()),
            throws: Some(Type::Enum("HttpError".to_string(), Vec::new())),
            capability: "Http".to_string(),
        },
        // The Socket capability (spec 0050): the raw TCP byte boundary that
        // backs the embedded `std.socket`. A primitive effect (spec 0049) over
        // wasi:sockets; HttpServer (spec 0046) is a derived effect on top.
        // `Listener`/`Connection`/`SocketError` are the named types declared by
        // `std.socket`. All but `close` are fallible (spec 0043).
        PlatformFn {
            path: vec!["socket".to_string()],
            name: "raw_listen".to_string(),
            params: vec![Type::Int],
            ret: Type::Enum("Listener".to_string(), Vec::new()),
            throws: Some(Type::Enum("SocketError".to_string(), Vec::new())),
            capability: "Socket".to_string(),
        },
        PlatformFn {
            path: vec!["socket".to_string()],
            name: "raw_accept".to_string(),
            params: vec![Type::Enum("Listener".to_string(), Vec::new())],
            ret: Type::Enum("Connection".to_string(), Vec::new()),
            throws: Some(Type::Enum("SocketError".to_string(), Vec::new())),
            capability: "Socket".to_string(),
        },
        PlatformFn {
            path: vec!["socket".to_string()],
            name: "raw_read".to_string(),
            params: vec![Type::Enum("Connection".to_string(), Vec::new()), Type::Int],
            ret: Type::Bytes,
            throws: Some(Type::Enum("SocketError".to_string(), Vec::new())),
            capability: "Socket".to_string(),
        },
        PlatformFn {
            path: vec!["socket".to_string()],
            name: "raw_write".to_string(),
            params: vec![
                Type::Enum("Connection".to_string(), Vec::new()),
                Type::Bytes,
            ],
            ret: Type::Unit,
            throws: Some(Type::Enum("SocketError".to_string(), Vec::new())),
            capability: "Socket".to_string(),
        },
        PlatformFn {
            path: vec!["socket".to_string()],
            name: "raw_close".to_string(),
            params: vec![Type::Int],
            ret: Type::Unit,
            throws: None,
            capability: "Socket".to_string(),
        },
        // The Random capability (spec 0054): a cryptographically-secure OS
        // entropy source backing the embedded `std.random`. A primitive effect
        // (spec 0049) over `wasi:random/random`. Both operations are infallible
        // (spec 0043): `raw_int` yields a uniform `Int`, `raw_bytes` a `Bytes` of
        // the requested length.
        PlatformFn {
            path: vec!["random".to_string()],
            name: "raw_int".to_string(),
            params: vec![],
            ret: Type::Int,
            throws: None,
            capability: "Random".to_string(),
        },
        PlatformFn {
            path: vec!["random".to_string()],
            name: "raw_bytes".to_string(),
            params: vec![Type::Int],
            ret: Type::Bytes,
            throws: None,
            capability: "Random".to_string(),
        },
    ]
}

/// Looks a platform function up by its canonical name (e.g. `io.write_stdout`).
pub fn lookup(canonical: &str) -> Option<PlatformFn> {
    platform_interface()
        .into_iter()
        .find(|entry| entry.canonical() == canonical)
}

/// Looks a platform function up in a given slice of entries (e.g. an extended
/// registry that includes host-interface entries — spec 0026).
pub fn lookup_in(entries: &[PlatformFn], canonical: &str) -> Option<PlatformFn> {
    entries
        .iter()
        .find(|entry| entry.canonical() == canonical)
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Socket capability (spec 0050) registers all five TCP operations
    /// under `socket.*`, each producing the `Socket` capability. `raw_close` is
    /// infallible; every other operation is fallible with `SocketError`.
    #[test]
    fn socket_registry_entries() {
        let socket_error = Some(Type::Enum("SocketError".to_string(), Vec::new()));
        let listener = Type::Enum("Listener".to_string(), Vec::new());
        let connection = Type::Enum("Connection".to_string(), Vec::new());

        let listen = lookup("socket.raw_listen").expect("socket.raw_listen registered");
        assert_eq!(listen.capability, "Socket");
        assert_eq!(listen.params, vec![Type::Int]);
        assert_eq!(listen.ret, listener);
        assert_eq!(listen.throws, socket_error);

        let accept = lookup("socket.raw_accept").expect("socket.raw_accept registered");
        assert_eq!(accept.capability, "Socket");
        assert_eq!(accept.params, vec![listener.clone()]);
        assert_eq!(accept.ret, connection);
        assert_eq!(accept.throws, socket_error);

        let read = lookup("socket.raw_read").expect("socket.raw_read registered");
        assert_eq!(read.capability, "Socket");
        assert_eq!(read.params, vec![connection.clone(), Type::Int]);
        assert_eq!(read.ret, Type::Bytes);
        assert_eq!(read.throws, socket_error);

        let write = lookup("socket.raw_write").expect("socket.raw_write registered");
        assert_eq!(write.capability, "Socket");
        assert_eq!(write.params, vec![connection, Type::Bytes]);
        assert_eq!(write.ret, Type::Unit);
        assert_eq!(write.throws, socket_error);

        // `close` takes an id (Int) and cannot fail (spec 0050 P2/P7).
        let close = lookup("socket.raw_close").expect("socket.raw_close registered");
        assert_eq!(close.capability, "Socket");
        assert_eq!(close.params, vec![Type::Int]);
        assert_eq!(close.ret, Type::Unit);
        assert_eq!(close.throws, None);
    }

    /// The Random capability (spec 0054) registers `raw_int` (no args → `Int`)
    /// and `raw_bytes` (`Int` → `Bytes`) under `random.*`, both infallible.
    #[test]
    fn random_registry_entries() {
        let raw_int = lookup("random.raw_int").expect("random.raw_int registered");
        assert_eq!(raw_int.capability, "Random");
        assert_eq!(raw_int.params, Vec::<Type>::new());
        assert_eq!(raw_int.ret, Type::Int);
        assert_eq!(raw_int.throws, None);

        let raw_bytes = lookup("random.raw_bytes").expect("random.raw_bytes registered");
        assert_eq!(raw_bytes.capability, "Random");
        assert_eq!(raw_bytes.params, vec![Type::Int]);
        assert_eq!(raw_bytes.ret, Type::Bytes);
        assert_eq!(raw_bytes.throws, None);
    }
}
