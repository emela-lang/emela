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
        // The HttpServer capability (spec 0046): a separate capability from the
        // client so a serve-only program's manifest shows no outbound access.
        PlatformFn {
            path: vec!["http".to_string()],
            name: "server_bind".to_string(),
            params: vec![Type::Int],
            ret: Type::Enum("Server".to_string(), Vec::new()),
            throws: Some(Type::Enum("HttpError".to_string(), Vec::new())),
            capability: "HttpServer".to_string(),
        },
        PlatformFn {
            path: vec!["http".to_string()],
            name: "server_accept".to_string(),
            params: vec![Type::Enum("Server".to_string(), Vec::new())],
            ret: Type::Enum("Incoming".to_string(), Vec::new()),
            throws: Some(Type::Enum("HttpError".to_string(), Vec::new())),
            capability: "HttpServer".to_string(),
        },
        PlatformFn {
            path: vec!["http".to_string()],
            name: "server_respond".to_string(),
            params: vec![
                Type::Enum("Incoming".to_string(), Vec::new()),
                Type::Enum("Response".to_string(), Vec::new()),
            ],
            ret: Type::Unit,
            throws: Some(Type::Enum("HttpError".to_string(), Vec::new())),
            capability: "HttpServer".to_string(),
        },
        PlatformFn {
            path: vec!["http".to_string()],
            name: "server_close".to_string(),
            params: vec![Type::Enum("Server".to_string(), Vec::new())],
            ret: Type::Unit,
            throws: Some(Type::Enum("HttpError".to_string(), Vec::new())),
            capability: "HttpServer".to_string(),
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
