//! Emits WASM from the Core IR.
//!
//! Does reference counting on linear memory (Perceus-style) and the state-machine transform for
//! `suspend` (spec 16.5). The WASM runtime is embedded and linked into the emitted module.
