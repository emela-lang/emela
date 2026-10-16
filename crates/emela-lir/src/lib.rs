//! The low IR (LIR) below the Core IR, for the WASM (and later C) backends.
//!
//! The low IR has two stages: the Flow IR (control made explicit) and the Heap IR (memory made
//! explicit: closure conversion, the suspend state machine and reference counting). This crate has
//! the Flow IR so far: structured ANF. Values are variables or constants; control is
//! explicit through `if`, `switch`, `loop` / `recur`, `block` / `break`, `protect` / `cleanup` and
//! terminators, with failure edges on fallible calls. Each variable has a representation type
//! ([`Repr`]) and its Core type (an `emela_core::TypeId` of the Core module the LIR module owns).
//! Closures keep their bodies nested, with explicit capture lists. Functions store their
//! variables, statements, sequences and labels in arenas, so analyses can keep results in side
//! tables keyed by ID.
//!
//! [`from_core`] converts monomorphized Core IR to the Flow IR. [`to_heap`] runs the passes of the
//! second stage, the Heap IR, on the same data types ([`Stage`] says which have run): closure
//! conversion, explicit defect paths and reference counting. The suspend state machine comes
//! later.

mod build;
mod dump;
mod from_core;
mod heap;
mod ir;
mod repr;
mod verify;

pub use build::FnBuilder;
pub use dump::{core_ty, dump, dump_function};
pub use from_core::{FromCoreError, from_core};
pub use heap::{
    builtin_may_defect, convert_closures, explicit_defects, fiber_may_defect, insert_rc, to_heap,
};
pub use ir::*;
pub use repr::{Repr, enum_repr, repr_of, repr_of_subst};
pub use verify::{ErrorKind, VerifyError, verify, verify_function};
