//! The single definition of the linear memory layout (scratch < static data < heap).
//!
//! The linear memory has three regions.
//!
//! ```text
//! [ scratch ][ static data ][ heap -> grow ]
//! 0          SCRATCH_END    heap_base
//! ```
//!
//! The boundaries of the regions are defined here, in one place. The scratch slots are declared
//! with the `scratch!` macro, and the end of the region (`SCRATCH_END`) is derived from that
//! table. Adding a slot moves the end with it, so the slots never overlap the static data.
//!
//! The emitted module follows from this definition, so a slot added anywhere else fails a
//! compile-time assertion or `tests/layout.rs`.

/// The page the bump allocator grows by.
pub const PAGE: u32 = 65536;

const fn align8(x: u32) -> u32 {
    (x + 7) & !7
}

/// The largest end of the slots, rounded up to 8 bytes.
const fn max_align8(ends: &[u32]) -> u32 {
    let mut m = 0u32;
    let mut i = 0;
    while i < ends.len() {
        if ends[i] > m {
            m = ends[i];
        }
        i += 1;
    }
    align8(m)
}

/// Declares the scratch slots. The offsets are chosen by hand, because they are baked into the
/// wasm as constants, but the end of the region (`SCRATCH_END`) is derived from this table.
macro_rules! scratch {
    ($($name:ident : $off:expr, $len:expr;)*) => {
        $(pub const $name: u32 = $off;)*
        pub const SCRATCH_END: u32 = max_align8(&[$($off + $len),*]);
    }
}

scratch! {
    WAITBUF:   0,   8;   // the event of waitable-set.wait (handle, payload)
    FUTUREBUF: 8,   8;   // the result of the future write-via-stream returns
    DIGITS:   64,  16;  // the digits of an Int (least significant first)
    NEWLINE:  88,  1;   // the newline of debug_print
    DIGITS64: 96,  24;  // the digits of an Int64
    JSONBUF:  128, 6;   // a short buffer for JSON escapes
    CPBUF:    160, 4;   // the UTF-8 of one code point
    HOSTBUF:  168, 16;  // a list or record returned by a WASI host
}

/// The start of the static data (right after the scratch region).
pub const STATIC_START: u32 = SCRATCH_END;

/// The start of the heap, after the static data.
pub(crate) fn heap_base(statics_len: u32) -> u32 {
    align8(statics_len).max(STATIC_START) + 8
}

/// The resolved layout (used by the tests and `WasmOutput`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub scratch: Region,
    pub statics: Region,
    pub heap_base: u32,
}

impl Layout {
    pub(crate) fn new(statics_len: u32) -> Self {
        Layout {
            scratch: Region {
                start: 0,
                end: SCRATCH_END,
            },
            statics: Region {
                start: STATIC_START,
                end: statics_len,
            },
            heap_base: heap_base(statics_len),
        }
    }
}

// Compile-time checks of the invariants
const _: () = assert!(STATIC_START.is_multiple_of(8));
const _: () = assert!(STATIC_START >= SCRATCH_END);
