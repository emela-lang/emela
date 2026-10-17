//! Every target (JS, C, WASM; spec chapter 16) is plugged into the pipeline through the same
//! target-agnostic traits. A refactor that renames or splits them — such as the one that renamed
//! `JsBackend`/`JsOutput` to `Backend`/`BuildOutput` — has to update every target, and this test
//! fails to compile until it does. That is the semantic conflict a rebase of one target's work
//! over such a rename runs into even though git reports no textual conflict.

use emela_driver::{Backend, CoreC, CoreJs, CoreModule, CoreWasm, TestBackend, TestModule};

/// Fails to compile unless `B` implements the output stage of `build`/`run`.
fn is_backend<B: Backend<CoreModule>>() {}

/// Fails to compile unless `B` implements the output stage of `test`.
fn is_test_backend<B: TestBackend<TestModule>>() {}

#[test]
fn every_target_uses_the_target_agnostic_traits() {
    is_backend::<CoreJs>();
    is_backend::<CoreC>();
    is_backend::<CoreWasm>();
    is_test_backend::<CoreJs>();
    is_test_backend::<CoreC>();
    is_test_backend::<CoreWasm>();
}
