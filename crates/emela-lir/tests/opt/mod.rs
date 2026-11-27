//! Tests of the LIR optimisation passes (`emela_lir::opt`).

mod borrow_call;
mod closure_sra;
mod dce;
mod devirt;
mod drop_spec;
mod frame_prune;
mod rc_borrow;
mod reuse;
mod sra;
mod sra_records;
