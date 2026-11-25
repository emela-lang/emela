//! Target-independent analyses of the LIR that the backends share.
//!
//! [`reach`] answers what the entry can reach: the functions whose bodies can run, the functions
//! whose address is named without being called, the handler operations some reachable code
//! performs, and the external functions reachable code calls. Both backends use it, so the set
//! that decides an emitted body and the set that decides a host import are the same analysis.
//!
//! This module is separate from [`crate::opt::analysis`], which holds the facts (escape,
//! liveness, borrow, ownership) that the optimization passes of [`crate::opt`] consume. A
//! reachability walk takes the entry roots as an argument, so it is a free function rather than
//! an `Analysis` with `analyze(&Module)`.

pub mod reach;
