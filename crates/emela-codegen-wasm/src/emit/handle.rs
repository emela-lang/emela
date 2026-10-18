//! Opaque host handles (`@external type`s) at the internal-host boundary.
//!
//! A `@external type` (`EnumDef::external`, spec 12.4) has no constructors and no observable
//! shape to the program: the backend only carries it around as a reference. An internal host
//! therefore crosses it as an `i32` — a pointer into linear memory — like any other `Repr::Ref`
//! value, and the emitter's reference counting treats it as an ordinary heap object.
//!
//! WASM's own host handle so far is `Std.FileSystem`'s `Outcome`. It is the one object the
//! program ever holds, and it is a plain Emela heap object (so `dup` and `drop` work) with five
//! word fields, none of them a reference:
//!
//! ```text
//! +16  error    one of the codes below (0 = success)
//! +24  message  an Emela String for `FileSystemFailure`, or 0
//! +32  text     an Emela String for `read_text`, or 0
//! +40  count    the number of names of `list_directory`
//! +48  names    a host buffer of `count` word slots, each an Emela String pointer
//! ```
//!
//! The strings and the name buffer inside an outcome are the host's own data: their slots are
//! marked as non-references, so `drop` frees only the outcome itself. The handler releases the
//! outcome through `release_outcome` (a no-op on WASM, where the process is short-lived) and the
//! accessors hand out fresh copies, exactly as the C host's `emela_fs_strdup` does.

use super::*;

/// The number of word fields of an outcome object (see the module comment).
pub(crate) const OUTCOME_SLOTS: u32 = 5;

/// The offsets of the outcome fields (the object header at `HDR`).
pub(crate) const OUTCOME_ERROR: u32 = HDR;
pub(crate) const OUTCOME_MESSAGE: u32 = HDR + 8;
pub(crate) const OUTCOME_TEXT: u32 = HDR + 16;
pub(crate) const OUTCOME_COUNT: u32 = HDR + 24;
pub(crate) const OUTCOME_NAMES: u32 = HDR + 32;

/// The success code (`Std.FileSystem`'s `check_*` match the empty name).
pub(crate) const OUTCOME_OK: i32 = 0;
/// The name of an error the operation's `fails` lists.
pub(crate) const OUTCOME_NOT_FOUND: i32 = 1;
pub(crate) const OUTCOME_ACCESS_DENIED: i32 = 2;
pub(crate) const OUTCOME_NOT_A_DIRECTORY: i32 = 3;
pub(crate) const OUTCOME_IS_A_DIRECTORY: i32 = 4;
pub(crate) const OUTCOME_NOT_EMPTY: i32 = 5;
pub(crate) const OUTCOME_INVALID_TEXT: i32 = 6;
/// Any other host error (the name `FileSystemFailure`).
pub(crate) const OUTCOME_FAILURE: i32 = 7;

/// The error name of a code (`""` for success) — the strings `Std.FileSystem`'s `check_*` match
/// on.
pub(crate) fn error_name(code: i32) -> &'static str {
    match code {
        OUTCOME_NOT_FOUND => "FileNotFound",
        OUTCOME_ACCESS_DENIED => "FileAccessDenied",
        OUTCOME_NOT_A_DIRECTORY => "NotADirectory",
        OUTCOME_IS_A_DIRECTORY => "IsADirectory",
        OUTCOME_NOT_EMPTY => "DirectoryNotEmpty",
        OUTCOME_INVALID_TEXT => "InvalidText",
        OUTCOME_FAILURE => "FileSystemFailure",
        _ => "",
    }
}

/// The descriptor of an outcome object: five word fields, none a reference.
pub(crate) fn outcome_desc(b: &mut Builder) -> u32 {
    b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Outcome".to_owned(),
        show: None,
        refs: vec![false; OUTCOME_SLOTS as usize],
    })
}

/// The descriptor of a raw host buffer (a stat record, the readdir buffer, the name array). Such
/// buffers carry no references and are never dropped (they are only ever held as words), so the
/// descriptor has no fields; it is just the marker `alloc` stores in the header.
pub(crate) fn buffer_desc(b: &mut Builder) -> u32 {
    b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "host-buffer".to_owned(),
        show: None,
        refs: Vec::new(),
    })
}
