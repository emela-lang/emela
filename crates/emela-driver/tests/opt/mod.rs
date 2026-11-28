//! The optimisations end to end: each one keeps what a program observes on every target.

mod algebra;
mod borrow_opt;
mod closure_sra;
mod drop_spec;
mod opt_diff;
mod prune;
mod record_sra;
mod reuse;
