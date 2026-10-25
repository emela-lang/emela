//! The WASI 0.3 host functions the component target knows how to call.
//!
//! A program's own `@external(wasm, ...)` is not a component import: a component can only import
//! interfaces its WIT world declares, and custom host WIT is not implemented. The bundled `Std`
//! modules do name a few WASI 0.3 functions (`Std.Console` stdout/stderr, `Std.Env`,
//! `Std.Random`, `Std.Clock.now` and `Std.Clock.sleep`); each of those externals is a runtime
//! wrapper with its own encoding of the host's result. `Std.FileSystem` names
//! `emela:runtime/filesystem@0.1.0`, a runtime label rather than a WIT interface: one operation
//! takes several `wasi:filesystem` calls, which the runtime imports itself. The entry adapter of
//! `Std.Http` names `emela:runtime/http@0.1.0` the same way, over `wasi:http/types`. The TestClock's
//! `when-stalled` is a scheduler hook of the runtime rather than a WIT import, and is recognized
//! the same way. [`classify`] recognizes them by their IR name, never by the module/name pair,
//! so a program that names the same WASI function stays an unsupported external in
//! `check/external.rs`.

use emela_lir::Function;

/// A supported WASI 0.3 host call, and the runtime wrapper that implements it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StdHost {
    /// `Std.Console.print_line`: a String, then a newline, on stdout.
    ConsoleOut,
    /// `Std.Console.print_error_line`: a String, then a newline, on stderr.
    ConsoleErr,
    /// `Std.Env.has`: whether the environment holds the name.
    EnvHas,
    /// `Std.Env.get`: the value of the name.
    EnvGet,
    /// `Std.Random.int`: 32 random bits.
    RandomInt,
    /// `Std.Random.float`: uniform in `[0, 1)`.
    RandomFloat,
    /// `Std.Clock.now`: Unix epoch milliseconds.
    ClockNow,
    /// `Std.Clock.sleep` on `SystemClock`: waits through the host's monotonic clock.
    ClockSleep,
    /// `Std.Clock.sleep` on `TestClock`: waits until the run stalls, without a host timer.
    ClockWhenStalled,
    /// `Std.FileSystem.read_text`: the text of a file.
    FsReadText,
    /// `Std.FileSystem.write_text`: replaces the text of a file.
    FsWriteText,
    /// `Std.FileSystem.append_text`: appends to a file.
    FsAppendText,
    /// `Std.FileSystem.exists`.
    FsExists,
    /// `Std.FileSystem.is_directory`.
    FsIsDirectory,
    /// `Std.FileSystem.list_directory`: the names in a directory.
    FsListDirectory,
    /// `Std.FileSystem.create_directory`: creates the parents too.
    FsCreateDirectory,
    /// `Std.FileSystem.remove`: a file or an empty directory.
    FsRemove,
    /// The error name of a host outcome.
    FsOutcomeError,
    /// The message of a host outcome.
    FsOutcomeMessage,
    /// The text of a host outcome.
    FsOutcomeText,
    /// The number of names of a host outcome.
    FsOutcomeNameCount,
    /// One name of a host outcome.
    FsOutcomeNameAt,
    /// Gives up a host outcome.
    FsReleaseOutcome,
    /// `Std.Http.serve`: reads the whole request being served.
    HttpRead,
    /// Whether the body of the request is UTF-8.
    HttpReceivedText,
    /// The method of the request.
    HttpReceivedMethod,
    /// The absolute URL of the request.
    HttpReceivedUrl,
    /// The body of the request.
    HttpReceivedBody,
    /// The number of headers of the request.
    HttpReceivedHeaderCount,
    /// One header name of the request.
    HttpReceivedHeaderName,
    /// One header value of the request.
    HttpReceivedHeaderValue,
    /// Starts the response: its status and body.
    HttpNewDraft,
    /// Adds a header to the response.
    HttpDraftHeader,
    /// The `response` resource of the response built.
    HttpFinishDraft,
}

impl StdHost {
    pub(crate) fn runtime_name(self) -> &'static str {
        match self {
            StdHost::ConsoleOut => "print_line",
            StdHost::ConsoleErr => "print_error_line",
            StdHost::EnvHas => "env_has",
            StdHost::EnvGet => "env_get",
            StdHost::RandomInt => "random_int",
            StdHost::RandomFloat => "random_float",
            StdHost::ClockNow => "clock_now",
            StdHost::ClockSleep => "sleep_start",
            StdHost::ClockWhenStalled => "clock_when_stalled",
            StdHost::FsReadText => "fs_read_text",
            StdHost::FsWriteText => "fs_write_text",
            StdHost::FsAppendText => "fs_append_text",
            StdHost::FsExists => "fs_exists",
            StdHost::FsIsDirectory => "fs_is_directory",
            StdHost::FsListDirectory => "fs_list_directory",
            StdHost::FsCreateDirectory => "fs_create_directory",
            StdHost::FsRemove => "fs_remove",
            StdHost::FsOutcomeError => "fs_outcome_error",
            StdHost::FsOutcomeMessage => "fs_outcome_message",
            StdHost::FsOutcomeText => "fs_outcome_text",
            StdHost::FsOutcomeNameCount => "fs_outcome_name_count",
            StdHost::FsOutcomeNameAt => "fs_outcome_name_at",
            StdHost::FsReleaseOutcome => "fs_release_outcome",
            StdHost::HttpRead => "http_read",
            StdHost::HttpReceivedText => "http_received_text",
            StdHost::HttpReceivedMethod => "http_received_method",
            StdHost::HttpReceivedUrl => "http_received_url",
            StdHost::HttpReceivedBody => "http_received_body",
            StdHost::HttpReceivedHeaderCount => "http_received_header_count",
            StdHost::HttpReceivedHeaderName => "http_received_header_name",
            StdHost::HttpReceivedHeaderValue => "http_received_header_value",
            StdHost::HttpNewDraft => "http_new_draft",
            StdHost::HttpDraftHeader => "http_draft_header",
            StdHost::HttpFinishDraft => "http_finish_draft",
        }
    }

    pub(crate) fn is_console(self) -> bool {
        matches!(self, StdHost::ConsoleOut | StdHost::ConsoleErr)
    }

    pub(crate) fn is_env(self) -> bool {
        matches!(self, StdHost::EnvHas | StdHost::EnvGet)
    }

    pub(crate) fn is_random(self) -> bool {
        matches!(self, StdHost::RandomInt | StdHost::RandomFloat)
    }

    pub(crate) fn is_clock(self) -> bool {
        matches!(self, StdHost::ClockNow)
    }

    pub(crate) fn is_sleep(self) -> bool {
        matches!(self, StdHost::ClockSleep | StdHost::ClockWhenStalled)
    }

    /// Whether the host needs the real monotonic clock import (not the TestClock hook).
    pub(crate) fn is_monotonic(self) -> bool {
        matches!(self, StdHost::ClockSleep)
    }

    /// Whether the host is part of `Std.Http.serve` (`runtime/http.rs`).
    pub(crate) fn is_http(self) -> bool {
        matches!(
            self,
            StdHost::HttpRead
                | StdHost::HttpReceivedText
                | StdHost::HttpReceivedMethod
                | StdHost::HttpReceivedUrl
                | StdHost::HttpReceivedBody
                | StdHost::HttpReceivedHeaderCount
                | StdHost::HttpReceivedHeaderName
                | StdHost::HttpReceivedHeaderValue
                | StdHost::HttpNewDraft
                | StdHost::HttpDraftHeader
                | StdHost::HttpFinishDraft
        )
    }

    /// Whether the host is part of `Std.FileSystem`.
    pub(crate) fn is_fs(self) -> bool {
        matches!(
            self,
            StdHost::FsReadText
                | StdHost::FsWriteText
                | StdHost::FsAppendText
                | StdHost::FsExists
                | StdHost::FsIsDirectory
                | StdHost::FsListDirectory
                | StdHost::FsCreateDirectory
                | StdHost::FsRemove
                | StdHost::FsOutcomeError
                | StdHost::FsOutcomeMessage
                | StdHost::FsOutcomeText
                | StdHost::FsOutcomeNameCount
                | StdHost::FsOutcomeNameAt
                | StdHost::FsReleaseOutcome
        )
    }
}

/// The bundled externals that are runtime wrappers, by IR name. `Std` cannot be the root name of
/// a project or a dependency (spec 4.1), so these names only come from the bundled sources.
const STD_HOSTS: &[(&str, StdHost)] = &[
    ("Std.Console.console_log", StdHost::ConsoleOut),
    ("Std.Console.console_error", StdHost::ConsoleErr),
    ("Std.Env.env_has", StdHost::EnvHas),
    ("Std.Env.env_get", StdHost::EnvGet),
    ("Std.Random.random_int", StdHost::RandomInt),
    ("Std.Random.random_float", StdHost::RandomFloat),
    ("Std.Clock.now_milliseconds", StdHost::ClockNow),
    ("Std.Clock.sleep_milliseconds", StdHost::ClockSleep),
    ("Std.Clock.when_stalled", StdHost::ClockWhenStalled),
    ("Std.FileSystem.host_read_text", StdHost::FsReadText),
    ("Std.FileSystem.host_write_text", StdHost::FsWriteText),
    ("Std.FileSystem.host_append_text", StdHost::FsAppendText),
    ("Std.FileSystem.host_exists", StdHost::FsExists),
    ("Std.FileSystem.host_is_directory", StdHost::FsIsDirectory),
    (
        "Std.FileSystem.host_list_directory",
        StdHost::FsListDirectory,
    ),
    (
        "Std.FileSystem.host_create_directory",
        StdHost::FsCreateDirectory,
    ),
    ("Std.FileSystem.host_remove", StdHost::FsRemove),
    ("Std.FileSystem.outcome_error", StdHost::FsOutcomeError),
    ("Std.FileSystem.outcome_message", StdHost::FsOutcomeMessage),
    ("Std.FileSystem.outcome_text", StdHost::FsOutcomeText),
    (
        "Std.FileSystem.outcome_name_count",
        StdHost::FsOutcomeNameCount,
    ),
    ("Std.FileSystem.outcome_name_at", StdHost::FsOutcomeNameAt),
    ("Std.FileSystem.release_outcome", StdHost::FsReleaseOutcome),
    ("Std.Http.host_read", StdHost::HttpRead),
    ("Std.Http.host_received_text", StdHost::HttpReceivedText),
    ("Std.Http.host_received_method", StdHost::HttpReceivedMethod),
    ("Std.Http.host_received_url", StdHost::HttpReceivedUrl),
    ("Std.Http.host_received_body", StdHost::HttpReceivedBody),
    (
        "Std.Http.host_received_header_count",
        StdHost::HttpReceivedHeaderCount,
    ),
    (
        "Std.Http.host_received_header_name",
        StdHost::HttpReceivedHeaderName,
    ),
    (
        "Std.Http.host_received_header_value",
        StdHost::HttpReceivedHeaderValue,
    ),
    ("Std.Http.host_draft", StdHost::HttpNewDraft),
    ("Std.Http.host_draft_header", StdHost::HttpDraftHeader),
    ("Std.Http.host_finish", StdHost::HttpFinishDraft),
];

/// If `f` is a bundled `Std` external with a `wasm` binding that the runtime implements, which
/// host call it is.
pub(crate) fn classify(f: &Function) -> Option<StdHost> {
    f.external.as_ref()?.binding("wasm")?;
    STD_HOSTS
        .iter()
        .find(|(name, _)| *name == f.name)
        .map(|&(_, host)| host)
}
