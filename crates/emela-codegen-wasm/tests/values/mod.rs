//! Records, tuples and arrays held by value: their WASM locals, the call ABIs and the boxes at
//! escape boundaries.

mod abi;
mod array_reads;
mod arrays;
mod boxes;
mod indirect;
mod locals;
mod suspend;
