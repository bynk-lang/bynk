//! In-process test helpers shared across `bynk-emit`'s `#[cfg(test)]` modules.
//!
//! Only the single-file path lives here. A test that needs the whole project pipeline
//! (cross-context `uses`, multi-file `Split` layouts, a suite beside its unit, a Workers build)
//! drives `bynk-driver`, and `bynk-emit` has no dependency on the driver of any kind (1B.2b,
//! #4028), so those tests live in `bynk-tests` (`tests/integration/`, over
//! `tests/support/emit_pipeline.rs`), where they assert on the same emitted text.

/// Compile a self-contained single-file commons and return its TypeScript.
/// Panics with the source on a compile failure — a test fixture that doesn't
/// compile is a bug in the test, not a case to assert on.
pub(crate) fn emit_source(src: &str) -> String {
    crate::compile(src, "t.bynk").expect("fixture should compile")
}
